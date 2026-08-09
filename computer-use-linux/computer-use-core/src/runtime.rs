//! The server-authoritative lease/snapshot/receipt runtime, mirroring
//! `ComputerUseRuntime.swift` and `RuntimeProtocols.swift`.
//!
//! The macOS runtime is an actor whose suspension points allow interleaving;
//! the Linux daemon instead serializes tool calls behind a mutex, which makes
//! every macOS interleaving guarantee hold trivially while keeping the same
//! fence/renewal checks in place at each step.

use serde_json::Value;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::args::ArgumentReader;
use crate::geometry;
use crate::models::{
    bounded_utf8, AccessibilityElementSnapshot, ActionReceipt, ActionReceiptState, AppDescriptor,
    AppTarget, CapturedDesktopState, ComputerUseError, GlobalScreenPoint, JsonObject, MouseButton,
    PngPixelPoint, ProtectedComputerUseCarrier, Result, SnapshotEnvelope, ToolCallContext,
    ToolExecutionResult, WindowGeometry,
};

pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

pub trait IdentifierGenerator: Send + Sync {
    fn next_identifier(&self) -> String;
}

pub struct UuidIdentifierGenerator;

impl IdentifierGenerator for UuidIdentifierGenerator {
    fn next_identifier(&self) -> String {
        Uuid::new_v4().to_string()
    }
}

pub trait ActionReceiptStore: Send + Sync {
    fn load(&self, identifier: &str) -> Result<Option<ActionReceipt>>;
    fn create(&self, receipt: &ActionReceipt) -> Result<()>;
    fn replace(&self, receipt: &ActionReceipt) -> Result<()>;
    /// Marks receipts stranded in `dispatched` as `outcome_unknown` (startup
    /// recovery after a crash between dispatch and its recorded outcome).
    fn recover_dispatched(&self, at: OffsetDateTime) -> Result<()>;
}

/// The native desktop bindings. Every input method must revalidate the exact
/// expected window (identifier, process, geometry) immediately before
/// dispatch and fail closed on any mismatch.
pub trait DesktopDriver: Send + Sync {
    fn list_apps(&self) -> Result<Vec<AppDescriptor>>;
    fn capture_by_bundle(
        &self,
        bundle_identifier: &str,
        window_identifier: Option<u32>,
    ) -> Result<CapturedDesktopState>;
    fn capture_by_process(
        &self,
        process_identifier: i32,
        window_identifier: Option<u32>,
    ) -> Result<CapturedDesktopState>;
    fn click(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        point: GlobalScreenPoint,
        button: MouseButton,
        count: u32,
    ) -> Result<()>;
    fn perform_accessibility_action(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        driver_token: &str,
        action: &str,
    ) -> Result<()>;
    fn scroll(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        point: GlobalScreenPoint,
        delta_x: f64,
        delta_y: f64,
    ) -> Result<()>;
    fn drag(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        from: GlobalScreenPoint,
        to: GlobalScreenPoint,
    ) -> Result<()>;
    fn type_text(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        text: &str,
    ) -> Result<()>;
    fn press_key(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        specification: &str,
    ) -> Result<()>;
    fn set_value(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        driver_token: &str,
        value: &str,
    ) -> Result<()>;
}

#[derive(Clone)]
struct Lease {
    owner_identifier: String,
    fence: u64,
    expires_at: OffsetDateTime,
}

#[derive(Clone)]
struct SnapshotRecord {
    envelope: SnapshotEnvelope,
    owner_identifier: String,
    fence: u64,
    consumed: bool,
    delivery_attested: bool,
}

struct InFlightOperation {
    owner_identifier: String,
    fence: u64,
    action_identifier: String,
}

pub struct ComputerUseRuntime {
    driver: Box<dyn DesktopDriver>,
    receipts: Box<dyn ActionReceiptStore>,
    clock: Box<dyn Clock>,
    identifiers: Box<dyn IdentifierGenerator>,
    lease_duration: Duration,
    lease: Option<Lease>,
    next_fence: u64,
    snapshots: std::collections::HashMap<String, SnapshotRecord>,
    in_flight_operation: Option<InFlightOperation>,
}

const MAX_OBSERVATION_BYTES: usize = 16 * 1024;
const MAX_TEXT_CHARS: usize = 32_768;

impl ComputerUseRuntime {
    pub fn new(
        driver: Box<dyn DesktopDriver>,
        receipts: Box<dyn ActionReceiptStore>,
        clock: Box<dyn Clock>,
        identifiers: Box<dyn IdentifierGenerator>,
        lease_duration_seconds: f64,
    ) -> Result<Self> {
        if !lease_duration_seconds.is_finite() || lease_duration_seconds <= 0.0 {
            return Err(ComputerUseError::InvalidArguments(
                "The desktop lease duration must be finite and positive.".to_owned(),
            ));
        }
        let now = clock.now();
        receipts.recover_dispatched(now)?;
        Ok(Self {
            driver,
            receipts,
            clock,
            identifiers,
            lease_duration: Duration::seconds_f64(lease_duration_seconds),
            lease: None,
            next_fence: 1,
            snapshots: std::collections::HashMap::new(),
            in_flight_operation: None,
        })
    }

    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> ToolExecutionResult {
        let result = match name {
            "list_apps" => self.list_apps(arguments),
            "get_app_state" => self.get_app_state(arguments, &context.client_identifier),
            "click" => self.click(arguments, context),
            "perform_secondary_action" => self.perform_secondary_action(arguments, context),
            "scroll" => self.scroll(arguments, context),
            "drag" => self.drag(arguments, context),
            "type_text" => self.type_text(arguments, context),
            "press_key" => self.press_key(arguments, context),
            "set_value" => self.set_value(arguments, context),
            "attest_snapshot_delivery" => self.attest_snapshot_delivery(arguments, context),
            "invalidate_session" => self.invalidate_session(arguments, context),
            "lease_heartbeat" => self.lease_heartbeat(arguments, context),
            "release_operation" => self.release_operation(arguments, context),
            _ => Err(ComputerUseError::InvalidArguments(format!(
                "Unknown tool: {name}"
            ))),
        };
        match result {
            Ok(result) => result,
            Err(error) => ToolExecutionResult::error(&error),
        }
    }

    pub fn action_outcome(&self, identifier: &str) -> Option<ActionReceipt> {
        self.receipts.load(identifier).ok().flatten()
    }

    pub fn client_disconnected(&mut self, identifier: &str) {
        if self
            .lease
            .as_ref()
            .is_some_and(|lease| lease.owner_identifier == identifier)
        {
            self.lease = None;
        }
        self.snapshots
            .retain(|_, record| record.owner_identifier != identifier);
    }

    fn list_apps(&mut self, arguments: JsonObject) -> Result<ToolExecutionResult> {
        ArgumentReader::new(arguments).finish()?;
        let apps = self.driver.list_apps()?;
        let mut text = format!("running_apps={}\n", apps.len());
        for app in &apps {
            let window_ids = app
                .window_identifiers
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let line = format!(
                "bundle_id={} pid={} active={} window_ids=[{}] name={}\n",
                app.bundle_identifier.as_deref().unwrap_or("<none>"),
                app.process_identifier,
                app.is_active,
                window_ids,
                app.name
            );
            if text.len() + line.len() > MAX_OBSERVATION_BYTES {
                break;
            }
            text.push_str(&line);
        }
        Ok(ToolExecutionResult::text(text))
    }

    fn get_app_state(
        &mut self,
        arguments: JsonObject,
        client_identifier: &str,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let bundle_identifier = reader.required_string("bundle_id", false)?;
        let requested_window = reader.optional_integer("window_id")?;
        reader.finish()?;
        let window_in_range = requested_window
            .map(|window| (1..=i64::from(u32::MAX)).contains(&window))
            .unwrap_or(true);
        if !(3..=255).contains(&bundle_identifier.len()) || !window_in_range {
            return Err(ComputerUseError::InvalidArguments(
                "bundle_id or window_id is outside the v2 contract.".to_owned(),
            ));
        }

        let active_lease = self.acquire_lease(client_identifier)?;
        let outcome = (|| {
            let captured = self.driver.capture_by_bundle(
                &bundle_identifier,
                requested_window.map(|window| window as u32),
            )?;
            let refreshed = self.renew_lease(client_identifier, active_lease.fence, true, None)?;
            let envelope = self.store_snapshot(captured, client_identifier, &refreshed);
            self.snapshot_result(&envelope, None)
        })();
        if outcome.is_err()
            && self.lease.as_ref().is_some_and(|lease| {
                lease.owner_identifier == client_identifier && lease.fence == active_lease.fence
            })
            && self
                .snapshots
                .values()
                .all(|record| record.owner_identifier != client_identifier)
        {
            self.lease = None;
        }
        outcome
    }

    fn click(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        let mut target_reader = ArgumentReader::new(reader.required_object("target")?);
        let kind = target_reader.required_string("kind", false)?;
        let button_name = reader
            .optional_string("button")?
            .unwrap_or_else(|| "left".to_owned());
        let count = reader.optional_integer_default("count", 1)?;
        reader.finish()?;

        let button = MouseButton::parse(&button_name)
            .filter(|button| *button != MouseButton::Middle)
            .ok_or_else(|| {
                ComputerUseError::InvalidArguments("button or count is invalid.".to_owned())
            })?;
        if !(1..=2).contains(&count) {
            return Err(ComputerUseError::InvalidArguments(
                "button or count is invalid.".to_owned(),
            ));
        }
        let count = count as u32;
        let captured = self
            .validated_snapshot(&snapshot_id, &context.client_identifier)?
            .envelope
            .captured
            .clone();

        if kind == "element" {
            let element_id = target_reader.required_string("element_id", false)?;
            target_reader.finish()?;
            if !(1..=128).contains(&element_id.len()) {
                return Err(ComputerUseError::InvalidArguments(
                    "element_id is outside the v2 contract.".to_owned(),
                ));
            }
            let element = find_element(&element_id, &captured)?.clone();
            let primary = element
                .actions
                .iter()
                .find(|action| action.eq_ignore_ascii_case("AXPress"))
                .cloned();
            if button == MouseButton::Left && count == 1 {
                if let Some(primary) = primary {
                    let app = captured.app.clone();
                    let expected = captured.geometry;
                    let token = element.driver_token.clone();
                    return self.dispatch_action(
                        "click",
                        &snapshot_id,
                        context,
                        move |driver, _| {
                            driver.perform_accessibility_action(&app, &expected, &token, &primary)
                        },
                    );
                }
            }
            let point = element
                .frame
                .as_ref()
                .map(|frame| frame.center())
                .ok_or_else(|| {
                    ComputerUseError::InvalidArguments(
                        "The selected element has no actionable frame.".to_owned(),
                    )
                })?;
            let app = captured.app.clone();
            let expected = captured.geometry;
            self.dispatch_action("click", &snapshot_id, context, move |driver, _| {
                driver.click(&app, &expected, point, button, count)
            })
        } else if kind == "pixel" {
            let pixel = PngPixelPoint {
                x: target_reader.required_number("x_px")?,
                y: target_reader.required_number("y_px")?,
            };
            target_reader.finish()?;
            let point = geometry::global_point(pixel, &captured.geometry)?;
            let app = captured.app.clone();
            let expected = captured.geometry;
            self.dispatch_action("click", &snapshot_id, context, move |driver, _| {
                driver.click(&app, &expected, point, button, count)
            })
        } else {
            Err(ComputerUseError::InvalidArguments(
                "target.kind must be element or pixel.".to_owned(),
            ))
        }
    }

    fn perform_secondary_action(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        let element_id = reader.required_string("element_id", false)?;
        let requested_action = reader.required_string("action_id", false)?;
        reader.finish()?;
        if !(1..=128).contains(&element_id.len()) || !(1..=128).contains(&requested_action.len()) {
            return Err(ComputerUseError::InvalidArguments(
                "element_id or action_id is outside the v2 contract.".to_owned(),
            ));
        }
        let captured = self
            .validated_snapshot(&snapshot_id, &context.client_identifier)?
            .envelope
            .captured
            .clone();
        let element = find_element(&element_id, &captured)?.clone();
        let action = element
            .actions
            .iter()
            .find(|action| action.eq_ignore_ascii_case(&requested_action))
            .filter(|action| !action.eq_ignore_ascii_case("AXPress"))
            .cloned()
            .ok_or_else(|| {
                ComputerUseError::InvalidArguments(
                    "The requested secondary action was not advertised by the snapshot.".to_owned(),
                )
            })?;
        let app = captured.app.clone();
        let expected = captured.geometry;
        let token = element.driver_token.clone();
        self.dispatch_action(
            "perform_secondary_action",
            &snapshot_id,
            context,
            move |driver, _| driver.perform_accessibility_action(&app, &expected, &token, &action),
        )
    }

    fn scroll(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        let element_id = reader.required_string("element_id", false)?;
        let direction = reader.required_string("direction", false)?;
        let pages = reader.optional_number("pages", 1.0)?;
        reader.finish()?;
        let pages_valid = pages > 0.0 && pages <= 10.0 && pages.is_finite();
        if !pages_valid || !["up", "down", "left", "right"].contains(&direction.as_str()) {
            return Err(ComputerUseError::InvalidArguments(
                "direction or pages is invalid.".to_owned(),
            ));
        }
        let captured = self
            .validated_snapshot(&snapshot_id, &context.client_identifier)?
            .envelope
            .captured
            .clone();
        let element = find_element(&element_id, &captured)?.clone();
        let point = element
            .frame
            .as_ref()
            .map(|frame| frame.center())
            .ok_or_else(|| {
                ComputerUseError::InvalidArguments(
                    "The selected element has no scroll point.".to_owned(),
                )
            })?;
        let magnitude = 12.0 * pages;
        let (delta_x, delta_y) = match direction.as_str() {
            "up" => (0.0, magnitude),
            "down" => (0.0, -magnitude),
            "left" => (magnitude, 0.0),
            _ => (-magnitude, 0.0),
        };
        let app = captured.app.clone();
        let expected = captured.geometry;
        self.dispatch_action("scroll", &snapshot_id, context, move |driver, _| {
            driver.scroll(&app, &expected, point, delta_x, delta_y)
        })
    }

    fn drag(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        let from = pixel_point(reader.required_object("from")?)?;
        let to = pixel_point(reader.required_object("to")?)?;
        reader.finish()?;
        let captured = self
            .validated_snapshot(&snapshot_id, &context.client_identifier)?
            .envelope
            .captured
            .clone();
        let global_from = geometry::global_point(from, &captured.geometry)?;
        let global_to = geometry::global_point(to, &captured.geometry)?;
        let app = captured.app.clone();
        let expected = captured.geometry;
        self.dispatch_action("drag", &snapshot_id, context, move |driver, _| {
            driver.drag(&app, &expected, global_from, global_to)
        })
    }

    fn type_text(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        let text = reader.required_string("text", true)?;
        reader.finish()?;
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(ComputerUseError::InvalidArguments(
                "text exceeds the v2 limit.".to_owned(),
            ));
        }
        self.dispatch_action("type_text", &snapshot_id, context, move |driver, record| {
            let captured = &record.envelope.captured;
            driver.type_text(&captured.app, &captured.geometry, &text)
        })
    }

    fn press_key(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        let key = reader.required_string("key", false)?;
        let modifiers = reader.optional_string_array("modifiers")?;
        reader.finish()?;
        let allowed = ["command", "control", "option", "shift", "fn"];
        let unique: std::collections::HashSet<&str> =
            modifiers.iter().map(String::as_str).collect();
        if unique.len() != modifiers.len()
            || !unique.iter().all(|modifier| allowed.contains(modifier))
        {
            return Err(ComputerUseError::InvalidArguments(
                "modifiers is invalid.".to_owned(),
            ));
        }
        let specification = modifiers
            .iter()
            .cloned()
            .chain(std::iter::once(key))
            .collect::<Vec<_>>()
            .join("+");
        self.dispatch_action("press_key", &snapshot_id, context, move |driver, record| {
            let captured = &record.envelope.captured;
            driver.press_key(&captured.app, &captured.geometry, &specification)
        })
    }

    fn set_value(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        let element_id = reader.required_string("element_id", false)?;
        let value = reader.required_string("value", true)?;
        reader.finish()?;
        if value.chars().count() > MAX_TEXT_CHARS {
            return Err(ComputerUseError::InvalidArguments(
                "value exceeds the v2 limit.".to_owned(),
            ));
        }
        let captured = self
            .validated_snapshot(&snapshot_id, &context.client_identifier)?
            .envelope
            .captured
            .clone();
        let element = find_element(&element_id, &captured)?.clone();
        if !element.is_value_settable {
            return Err(ComputerUseError::InvalidArguments(
                "The selected element was not settable in the snapshot.".to_owned(),
            ));
        }
        let app = captured.app.clone();
        let expected = captured.geometry;
        let token = element.driver_token.clone();
        self.dispatch_action("set_value", &snapshot_id, context, move |driver, _| {
            driver.set_value(&app, &expected, &token, &value)
        })
    }

    fn attest_snapshot_delivery(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        let attestation_id = reader.required_string("attestation_id", false)?;
        let png_sha256 = reader.required_string("png_sha256", false)?;
        reader.finish()?;
        let now = self.clock.now();
        let lease = self.lease.clone();
        let record = self.snapshots.get_mut(&snapshot_id);
        let valid = record.as_ref().is_some_and(|record| {
            record.owner_identifier == context.client_identifier
                && record.envelope.delivery_attestation_identifier == attestation_id
                && record.envelope.captured.screenshot_sha256 == png_sha256
                && !record.consumed
                && lease.as_ref().is_some_and(|lease| {
                    lease.owner_identifier == context.client_identifier
                        && lease.fence == record.fence
                        && lease.expires_at > now
                })
        });
        if !valid {
            return Err(ComputerUseError::InvalidSnapshot);
        }
        record.expect("validated above").delivery_attested = true;
        Ok(ToolExecutionResult::text("Snapshot delivery attested."))
    }

    fn invalidate_session(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        ArgumentReader::new(arguments).finish()?;
        self.snapshots
            .retain(|_, record| record.owner_identifier != context.client_identifier);
        if self
            .lease
            .as_ref()
            .is_some_and(|lease| lease.owner_identifier == context.client_identifier)
        {
            self.lease = None;
        }
        Ok(ToolExecutionResult::text(
            "Computer-use session invalidated.",
        ))
    }

    fn lease_heartbeat(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        reader.finish()?;
        let now = self.clock.now();
        let fence = {
            let record = self.snapshots.get(&snapshot_id);
            let valid = record.is_some_and(|record| {
                record.owner_identifier == context.client_identifier
                    && self.lease.as_ref().is_some_and(|lease| {
                        lease.owner_identifier == context.client_identifier
                            && lease.fence == record.fence
                            && lease.expires_at > now
                    })
            });
            if !valid {
                return Err(ComputerUseError::InvalidSnapshot);
            }
            record.expect("validated above").fence
        };
        self.renew_lease(&context.client_identifier, fence, true, None)?;
        Ok(ToolExecutionResult::text("Desktop lease renewed."))
    }

    fn release_operation(
        &mut self,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> Result<ToolExecutionResult> {
        let mut reader = ArgumentReader::new(arguments);
        let snapshot_id = reader.required_string("snapshot_id", false)?;
        reader.finish()?;
        let valid = self.snapshots.get(&snapshot_id).is_some_and(|record| {
            record.owner_identifier == context.client_identifier
                && self.lease.as_ref().is_some_and(|lease| {
                    lease.owner_identifier == context.client_identifier
                        && lease.fence == record.fence
                })
                && self.in_flight_operation.is_none()
        });
        if !valid {
            return Err(ComputerUseError::InvalidSnapshot);
        }
        self.snapshots.remove(&snapshot_id);
        if self
            .lease
            .as_ref()
            .is_some_and(|lease| lease.owner_identifier == context.client_identifier)
        {
            self.lease = None;
        }
        Ok(ToolExecutionResult::text("Desktop operation released."))
    }

    fn dispatch_action<F>(
        &mut self,
        tool_name: &str,
        snapshot_id: &str,
        context: &ToolCallContext,
        operation: F,
    ) -> Result<ToolExecutionResult>
    where
        F: FnOnce(&dyn DesktopDriver, &SnapshotRecord) -> Result<()>,
    {
        let action_id = context
            .action_identifier
            .as_deref()
            .filter(|identifier| {
                !identifier.is_empty()
                    && identifier.len() <= 256
                    && !identifier.chars().any(char::is_control)
            })
            .ok_or_else(|| {
                ComputerUseError::InternalFailure(
                    "The relay did not supply a valid action identifier.".to_owned(),
                )
            })?
            .to_owned();

        let outcome =
            self.dispatch_action_inner(tool_name, snapshot_id, context, &action_id, operation);
        if self
            .in_flight_operation
            .as_ref()
            .is_some_and(|operation| operation.action_identifier == action_id)
        {
            self.in_flight_operation = None;
        }
        outcome
    }

    fn dispatch_action_inner<F>(
        &mut self,
        tool_name: &str,
        snapshot_id: &str,
        context: &ToolCallContext,
        action_id: &str,
        operation: F,
    ) -> Result<ToolExecutionResult>
    where
        F: FnOnce(&dyn DesktopDriver, &SnapshotRecord) -> Result<()>,
    {
        if let Some(existing) = self.receipts.load(action_id)? {
            if existing.tool_name != tool_name || existing.snapshot_identifier != snapshot_id {
                return Err(ComputerUseError::InvalidArguments(
                    "The action identifier conflicts with an existing receipt.".to_owned(),
                ));
            }
            match existing.state {
                ActionReceiptState::Applied => {
                    return Ok(ToolExecutionResult::text(format!(
                        "Action receipt {} is already applied; it was not dispatched again. Call get_app_state.",
                        existing.identifier
                    )));
                }
                ActionReceiptState::Rejected => {
                    return Ok(ToolExecutionResult {
                        text: format!(
                            "The action was rejected before dispatch. Receipt state: {}.",
                            existing.state.as_str()
                        ),
                        structured_content: None,
                        image_png: None,
                        is_error: true,
                        protected_carrier: None,
                    });
                }
                ActionReceiptState::Dispatched | ActionReceiptState::OutcomeUnknown => {
                    return Err(ComputerUseError::ActionOutcomeUnknown(
                        "The action may have been applied and will not be retried. Call get_app_state to observe the desktop.".to_owned(),
                    ));
                }
                ActionReceiptState::Prepared => {}
            }
        }

        let record = self
            .validated_snapshot(snapshot_id, &context.client_identifier)?
            .clone();
        let now = self.clock.now();
        if self.receipts.load(action_id)?.is_none() {
            self.receipts.create(&ActionReceipt {
                identifier: action_id.to_owned(),
                tool_name: tool_name.to_owned(),
                snapshot_identifier: snapshot_id.to_owned(),
                state: ActionReceiptState::Prepared,
                created_at: now,
                updated_at: now,
                failure_code: None,
            })?;
        }

        let prepared = self.required_receipt(action_id)?;
        self.receipts
            .replace(&prepared.transitioned(ActionReceiptState::Dispatched, self.clock.now(), None))
            .map_err(|_| {
                ComputerUseError::InternalFailure(
                    "The action was not sent because its durable dispatch record could not be written.".to_owned(),
                )
            })?;

        self.snapshots.remove(snapshot_id);
        self.in_flight_operation = Some(InFlightOperation {
            owner_identifier: context.client_identifier.clone(),
            fence: record.fence,
            action_identifier: action_id.to_owned(),
        });
        self.renew_lease(
            &context.client_identifier,
            record.fence,
            false,
            Some(action_id),
        )?;

        if let Err(_dispatch_error) = operation(self.driver.as_ref(), &record) {
            if let Ok(dispatched) = self.required_receipt(action_id) {
                let _ = self.receipts.replace(&dispatched.transitioned(
                    ActionReceiptState::OutcomeUnknown,
                    self.clock.now(),
                    Some("dispatch_error".to_owned()),
                ));
            }
            return Err(ComputerUseError::ActionOutcomeUnknown(
                "The input dispatch did not produce a certain outcome and will not be retried. Call get_app_state to observe the desktop.".to_owned(),
            ));
        }

        let lease_remained_reserved = self
            .renew_lease(
                &context.client_identifier,
                record.fence,
                false,
                Some(action_id),
            )
            .is_ok();

        let dispatched = self.required_receipt(action_id)?;
        self.receipts
            .replace(&dispatched.transitioned(ActionReceiptState::Applied, self.clock.now(), None))
            .map_err(|_| {
                ComputerUseError::ActionOutcomeUnknown(
                    "The input was sent but its applied receipt could not be made durable. It will not be retried.".to_owned(),
                )
            })?;

        if !lease_remained_reserved {
            return Ok(ToolExecutionResult::text(
                "Action applied, but the desktop lease changed before the follow-up screenshot. Call get_app_state.",
            ));
        }

        let follow_up = (|| -> Result<ToolExecutionResult> {
            let captured = self.driver.capture_by_process(
                record.envelope.captured.app.process_identifier,
                Some(record.envelope.captured.geometry.window_identifier),
            )?;
            let current_lease = self.renew_lease(
                &context.client_identifier,
                record.fence,
                false,
                Some(action_id),
            )?;
            let next = self.store_snapshot(captured, &context.client_identifier, &current_lease);
            self.snapshot_result(&next, Some(action_id))
        })();
        Ok(follow_up.unwrap_or_else(|_| {
            ToolExecutionResult::text(
                "Action applied, but the follow-up screenshot failed. Call get_app_state before the next action.",
            )
        }))
    }

    fn acquire_lease(&mut self, owner_identifier: &str) -> Result<Lease> {
        let now = self.clock.now();
        if self.in_flight_operation.is_some() {
            return Err(ComputerUseError::DesktopBusy {
                retry_after_milliseconds: 250,
            });
        }
        if let Some(current) = &self.lease {
            if current.expires_at > now && current.owner_identifier != owner_identifier {
                let remaining = (current.expires_at - now).whole_milliseconds();
                let retry = remaining.max(1) as u64;
                return Err(ComputerUseError::DesktopBusy {
                    retry_after_milliseconds: retry,
                });
            }
        }
        self.snapshots.clear();
        self.issue_lease(owner_identifier, now)
    }

    fn issue_lease(&mut self, owner_identifier: &str, at: OffsetDateTime) -> Result<Lease> {
        if self.next_fence == u64::MAX {
            return Err(ComputerUseError::InternalFailure(
                "The desktop lease fence space is exhausted.".to_owned(),
            ));
        }
        let acquired = Lease {
            owner_identifier: owner_identifier.to_owned(),
            fence: self.next_fence,
            expires_at: at + self.lease_duration,
        };
        self.next_fence += 1;
        self.lease = Some(acquired.clone());
        Ok(acquired)
    }

    fn renew_lease(
        &mut self,
        owner_identifier: &str,
        fence: u64,
        require_unexpired: bool,
        reserved_by: Option<&str>,
    ) -> Result<Lease> {
        let now = self.clock.now();
        let valid = self.lease.as_ref().is_some_and(|current| {
            current.owner_identifier == owner_identifier
                && current.fence == fence
                && (!require_unexpired || current.expires_at > now)
        });
        if !valid {
            return Err(ComputerUseError::StateUnavailable(
                "The desktop lease changed while the operation was in progress.".to_owned(),
            ));
        }
        if let Some(action_identifier) = reserved_by {
            let reserved = self.in_flight_operation.as_ref().is_some_and(|operation| {
                operation.owner_identifier == owner_identifier
                    && operation.fence == fence
                    && operation.action_identifier == action_identifier
            });
            if !reserved {
                return Err(ComputerUseError::StateUnavailable(
                    "The desktop operation reservation was lost.".to_owned(),
                ));
            }
        }
        let current = self.lease.as_mut().expect("validated above");
        current.expires_at = now + self.lease_duration;
        Ok(current.clone())
    }

    fn store_snapshot(
        &mut self,
        captured: CapturedDesktopState,
        owner_identifier: &str,
        lease: &Lease,
    ) -> SnapshotEnvelope {
        self.snapshots
            .retain(|_, record| record.owner_identifier != owner_identifier);
        let envelope = SnapshotEnvelope {
            snapshot_identifier: self.identifiers.next_identifier(),
            delivery_attestation_identifier: self.identifiers.next_identifier(),
            captured,
            lease_expires_at: lease.expires_at,
        };
        self.snapshots.insert(
            envelope.snapshot_identifier.clone(),
            SnapshotRecord {
                envelope: envelope.clone(),
                owner_identifier: owner_identifier.to_owned(),
                fence: lease.fence,
                consumed: false,
                delivery_attested: false,
            },
        );
        envelope
    }

    fn validated_snapshot(
        &self,
        identifier: &str,
        owner_identifier: &str,
    ) -> Result<&SnapshotRecord> {
        let record = self
            .snapshots
            .get(identifier)
            .filter(|record| record.owner_identifier == owner_identifier)
            .ok_or(ComputerUseError::InvalidSnapshot)?;
        if record.consumed {
            return Err(ComputerUseError::SnapshotConsumed);
        }
        if !record.delivery_attested {
            return Err(ComputerUseError::InvalidSnapshot);
        }
        let now = self.clock.now();
        let lease_valid = self.lease.as_ref().is_some_and(|lease| {
            lease.owner_identifier == owner_identifier
                && lease.fence == record.fence
                && lease.expires_at > now
        });
        if !lease_valid {
            return Err(ComputerUseError::InvalidSnapshot);
        }
        Ok(record)
    }

    fn required_receipt(&self, identifier: &str) -> Result<ActionReceipt> {
        self.receipts.load(identifier)?.ok_or_else(|| {
            ComputerUseError::InternalFailure(
                "The durable action receipt is unavailable.".to_owned(),
            )
        })
    }

    fn snapshot_result(
        &self,
        envelope: &SnapshotEnvelope,
        receipt_identifier: Option<&str>,
    ) -> Result<ToolExecutionResult> {
        let captured = &envelope.captured;
        let mut header = format!("snapshot_id={}\n", envelope.snapshot_identifier);
        if let Some(receipt_identifier) = receipt_identifier {
            header.push_str(&format!(
                "receipt_id={receipt_identifier} action_state=applied\n"
            ));
        }
        header.push_str(&format!(
            "Coordinates use continuous PNG edge-space: width={}, height={}, origin=(0,0) top-left, x right, y down; require 0<=x<width and 0<=y<height.\n",
            captured.geometry.png_width_pixels, captured.geometry.png_height_pixels
        ));
        let text = bounded_utf8(
            &format!("{header}{}", captured.accessibility_tree),
            MAX_OBSERVATION_BYTES,
        );
        Ok(ToolExecutionResult {
            text,
            structured_content: None,
            image_png: Some(captured.screenshot_png.clone()),
            is_error: false,
            protected_carrier: Some(ProtectedComputerUseCarrier::new(envelope)?),
        })
    }
}

fn find_element<'captured>(
    identifier: &str,
    captured: &'captured CapturedDesktopState,
) -> Result<&'captured AccessibilityElementSnapshot> {
    captured
        .elements
        .iter()
        .find(|element| element.identifier == identifier)
        .ok_or_else(|| {
            ComputerUseError::InvalidArguments(
                "The element identifier is not present in this snapshot.".to_owned(),
            )
        })
}

fn pixel_point(object: JsonObject) -> Result<PngPixelPoint> {
    let mut reader = ArgumentReader::new(object);
    let point = PngPixelPoint {
        x: reader.required_number("x_px")?,
        y: reader.required_number("y_px")?,
    };
    reader.finish()?;
    Ok(point)
}

pub fn arguments_from_value(value: Value) -> Result<JsonObject> {
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(ComputerUseError::InvalidArguments(
            "arguments must be an object.".to_owned(),
        )),
    }
}
