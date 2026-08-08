//! Runtime state-machine tests mirroring the macOS `RuntimeTests.swift`
//! scenarios, adapted to the serialized (mutex-guarded) Linux runtime.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use computer_use_core::models::*;
use computer_use_core::runtime::*;
use serde_json::json;
use time::{Duration, OffsetDateTime};

#[derive(Default)]
struct MemoryReceiptStore {
    values: Mutex<HashMap<String, ActionReceipt>>,
}

impl MemoryReceiptStore {
    fn receipt(&self, identifier: &str) -> Option<ActionReceipt> {
        self.values.lock().unwrap().get(identifier).cloned()
    }

    fn any_dispatched(&self) -> bool {
        self.values
            .lock()
            .unwrap()
            .values()
            .any(|receipt| receipt.state == ActionReceiptState::Dispatched)
    }
}

impl ActionReceiptStore for &'static MemoryReceiptStore {
    fn load(&self, identifier: &str) -> Result<Option<ActionReceipt>> {
        Ok(self.receipt(identifier))
    }

    fn create(&self, receipt: &ActionReceipt) -> Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(receipt.identifier.clone(), receipt.clone());
        Ok(())
    }

    fn replace(&self, receipt: &ActionReceipt) -> Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(receipt.identifier.clone(), receipt.clone());
        Ok(())
    }

    fn recover_dispatched(&self, at: OffsetDateTime) -> Result<()> {
        let mut values = self.values.lock().unwrap();
        let stranded: Vec<String> = values
            .iter()
            .filter(|(_, receipt)| receipt.state == ActionReceiptState::Dispatched)
            .map(|(identifier, _)| identifier.clone())
            .collect();
        for identifier in stranded {
            let receipt = values[&identifier].clone();
            values.insert(
                identifier,
                receipt.transitioned(ActionReceiptState::OutcomeUnknown, at, None),
            );
        }
        Ok(())
    }
}

struct FakeDesktopDriver {
    receipts: &'static MemoryReceiptStore,
    click_count: AtomicU32,
    fail_click: AtomicBool,
    fail_capture: AtomicBool,
    saw_durable_dispatch_before_effect: AtomicBool,
}

impl FakeDesktopDriver {
    fn new(receipts: &'static MemoryReceiptStore) -> Self {
        Self {
            receipts,
            click_count: AtomicU32::new(0),
            fail_click: AtomicBool::new(false),
            fail_capture: AtomicBool::new(false),
            saw_durable_dispatch_before_effect: AtomicBool::new(false),
        }
    }

    fn fixture_state(&self) -> Result<CapturedDesktopState> {
        if self.fail_capture.load(Ordering::SeqCst) {
            return Err(ComputerUseError::StateUnavailable(
                "synthetic capture failure".to_owned(),
            ));
        }
        Ok(CapturedDesktopState {
            app: AppTarget {
                name: "Fixture".to_owned(),
                bundle_identifier: Some("com.example.fixture".to_owned()),
                process_identifier: 42,
            },
            window_title: Some("Fixture".to_owned()),
            geometry: WindowGeometry {
                window_identifier: 7,
                global_bounds_points: GlobalScreenRect {
                    x: 100.0,
                    y: 200.0,
                    width: 400.0,
                    height: 200.0,
                },
                png_width_pixels: 800,
                png_height_pixels: 400,
            },
            screenshot_png: vec![0x89, 0x50, 0x4e, 0x47],
            screenshot_sha256: "0".repeat(64),
            accessibility_tree: "[e1] AXButton label=\"Go\" actions=AXPress".to_owned(),
            elements: vec![AccessibilityElementSnapshot {
                identifier: "e1".to_owned(),
                role: "AXButton".to_owned(),
                label: Some("Go".to_owned()),
                value: None,
                frame: Some(GlobalScreenRect {
                    x: 120.0,
                    y: 220.0,
                    width: 40.0,
                    height: 20.0,
                }),
                actions: vec!["AXPress".to_owned()],
                is_value_settable: false,
                driver_token: "driver-e1".to_owned(),
            }],
        })
    }
}

impl DesktopDriver for &'static FakeDesktopDriver {
    fn list_apps(&self) -> Result<Vec<AppDescriptor>> {
        Ok(vec![AppDescriptor {
            name: "Fixture".to_owned(),
            bundle_identifier: Some("com.example.fixture".to_owned()),
            process_identifier: 42,
            is_active: true,
            window_identifiers: vec![7],
        }])
    }

    fn capture_by_bundle(
        &self,
        _bundle: &str,
        _window: Option<u32>,
    ) -> Result<CapturedDesktopState> {
        self.fixture_state()
    }

    fn capture_by_process(&self, _pid: i32, _window: Option<u32>) -> Result<CapturedDesktopState> {
        self.fixture_state()
    }

    fn click(
        &self,
        _app: &AppTarget,
        _expected: &WindowGeometry,
        _point: GlobalScreenPoint,
        _button: MouseButton,
        _count: u32,
    ) -> Result<()> {
        self.click_count.fetch_add(1, Ordering::SeqCst);
        if self.receipts.any_dispatched() {
            self.saw_durable_dispatch_before_effect
                .store(true, Ordering::SeqCst);
        }
        if self.fail_click.load(Ordering::SeqCst) {
            return Err(ComputerUseError::StateUnavailable(
                "synthetic uncertain dispatch".to_owned(),
            ));
        }
        Ok(())
    }

    fn perform_accessibility_action(
        &self,
        _app: &AppTarget,
        _expected: &WindowGeometry,
        _token: &str,
        _action: &str,
    ) -> Result<()> {
        Ok(())
    }

    fn scroll(
        &self,
        _app: &AppTarget,
        _expected: &WindowGeometry,
        _point: GlobalScreenPoint,
        _delta_x: f64,
        _delta_y: f64,
    ) -> Result<()> {
        Ok(())
    }

    fn drag(
        &self,
        _app: &AppTarget,
        _expected: &WindowGeometry,
        _from: GlobalScreenPoint,
        _to: GlobalScreenPoint,
    ) -> Result<()> {
        Ok(())
    }

    fn type_text(&self, _app: &AppTarget, _expected: &WindowGeometry, _text: &str) -> Result<()> {
        Ok(())
    }

    fn press_key(&self, _app: &AppTarget, _expected: &WindowGeometry, _spec: &str) -> Result<()> {
        Ok(())
    }

    fn set_value(
        &self,
        _app: &AppTarget,
        _expected: &WindowGeometry,
        _token: &str,
        _value: &str,
    ) -> Result<()> {
        Ok(())
    }
}

struct MutableClock {
    now: Mutex<OffsetDateTime>,
}

impl MutableClock {
    fn new() -> Self {
        Self {
            now: Mutex::new(OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()),
        }
    }

    fn advance(&self, seconds: f64) {
        let mut now = self.now.lock().unwrap();
        *now += Duration::seconds_f64(seconds);
    }
}

impl Clock for &'static MutableClock {
    fn now(&self) -> OffsetDateTime {
        *self.now.lock().unwrap()
    }
}

struct SequenceIdentifiers {
    counter: Mutex<u64>,
}

impl IdentifierGenerator for SequenceIdentifiers {
    fn next_identifier(&self) -> String {
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;
        format!("00000000-0000-4000-8000-{:012}", *counter)
    }
}

struct Fixture {
    runtime: ComputerUseRuntime,
    receipts: &'static MemoryReceiptStore,
    driver: &'static FakeDesktopDriver,
    clock: &'static MutableClock,
}

fn fixture() -> Fixture {
    let receipts: &'static MemoryReceiptStore = Box::leak(Box::default());
    let driver: &'static FakeDesktopDriver = Box::leak(Box::new(FakeDesktopDriver::new(receipts)));
    let clock: &'static MutableClock = Box::leak(Box::new(MutableClock::new()));
    let runtime = ComputerUseRuntime::new(
        Box::new(driver),
        Box::new(receipts),
        Box::new(clock),
        Box::new(SequenceIdentifiers {
            counter: Mutex::new(0),
        }),
        30.0,
    )
    .unwrap();
    Fixture {
        runtime,
        receipts,
        driver,
        clock,
    }
}

fn context(client: &str) -> ToolCallContext {
    ToolCallContext {
        client_identifier: client.to_owned(),
        action_identifier: None,
    }
}

fn action_context(client: &str, action: &str) -> ToolCallContext {
    ToolCallContext {
        client_identifier: client.to_owned(),
        action_identifier: Some(action.to_owned()),
    }
}

fn arguments(value: serde_json::Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

fn get_app_state(fixture: &mut Fixture, client: &str) -> ToolExecutionResult {
    fixture.runtime.call_tool(
        "get_app_state",
        arguments(json!({"bundle_id": "com.example.fixture"})),
        &context(client),
    )
}

fn attest(
    fixture: &mut Fixture,
    carrier: &ProtectedComputerUseCarrier,
    client: &str,
) -> ToolExecutionResult {
    fixture.runtime.call_tool(
        "attest_snapshot_delivery",
        arguments(json!({
            "snapshot_id": carrier.snapshot_id,
            "attestation_id": carrier.attestation_id,
            "png_sha256": carrier.png_sha256,
        })),
        &context(client),
    )
}

fn pixel_click(snapshot_id: &str) -> JsonObject {
    arguments(json!({
        "snapshot_id": snapshot_id,
        "target": {"kind": "pixel", "x_px": 10, "y_px": 10},
    }))
}

#[test]
fn snapshot_requires_delivery_attestation() {
    let mut fixture = fixture();
    let state = get_app_state(&mut fixture, "client-a");
    let carrier = state.protected_carrier.clone().unwrap();

    let rejected = fixture.runtime.call_tool(
        "click",
        pixel_click(&carrier.snapshot_id),
        &action_context("client-a", "action-before-attestation"),
    );
    assert!(rejected.is_error);
    assert_eq!(fixture.driver.click_count.load(Ordering::SeqCst), 0);

    let attested = attest(&mut fixture, &carrier, "client-a");
    assert!(!attested.is_error);

    let clicked = fixture.runtime.call_tool(
        "click",
        pixel_click(&carrier.snapshot_id),
        &action_context("client-a", "action-after-attestation"),
    );
    assert!(
        !clicked.is_error,
        "click after attestation failed: {}",
        clicked.text
    );
    assert_eq!(fixture.driver.click_count.load(Ordering::SeqCst), 1);
    assert!(fixture
        .driver
        .saw_durable_dispatch_before_effect
        .load(Ordering::SeqCst));
}

#[test]
fn consumed_snapshot_rejects_a_second_action() {
    let mut fixture = fixture();
    let state = get_app_state(&mut fixture, "client-a");
    let carrier = state.protected_carrier.clone().unwrap();
    attest(&mut fixture, &carrier, "client-a");

    let first = fixture.runtime.call_tool(
        "click",
        pixel_click(&carrier.snapshot_id),
        &action_context("client-a", "action-one"),
    );
    let second = fixture.runtime.call_tool(
        "click",
        pixel_click(&carrier.snapshot_id),
        &action_context("client-a", "action-two"),
    );

    assert!(!first.is_error);
    assert!(
        second.is_error,
        "consumed snapshot must not authorize another action"
    );
    assert_eq!(fixture.driver.click_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.receipts.receipt("action-one").unwrap().state,
        ActionReceiptState::Applied
    );
    assert!(fixture.receipts.receipt("action-two").is_none());
}

#[test]
fn dispatch_failure_becomes_outcome_unknown_and_is_never_retried() {
    let mut fixture = fixture();
    fixture.driver.fail_click.store(true, Ordering::SeqCst);
    let state = get_app_state(&mut fixture, "client-a");
    let carrier = state.protected_carrier.clone().unwrap();
    attest(&mut fixture, &carrier, "client-a");
    let context = action_context("client-a", "uncertain-action");

    let first = fixture
        .runtime
        .call_tool("click", pixel_click(&carrier.snapshot_id), &context);
    let second = fixture
        .runtime
        .call_tool("click", pixel_click(&carrier.snapshot_id), &context);

    assert!(first.is_error);
    assert!(first.text.contains("outcome_unknown"));
    assert!(second.is_error);
    assert_eq!(fixture.driver.click_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.receipts.receipt("uncertain-action").unwrap().state,
        ActionReceiptState::OutcomeUnknown
    );
}

#[test]
fn desktop_lease_fences_other_clients() {
    let mut fixture = fixture();
    let first = get_app_state(&mut fixture, "client-a");
    assert!(!first.is_error);
    let busy = get_app_state(&mut fixture, "client-b");
    assert!(busy.is_error);
    assert!(busy.text.contains("desktop_busy"));

    // After expiry another client can take the lease, invalidating old snapshots.
    fixture.clock.advance(31.0);
    let taken = get_app_state(&mut fixture, "client-b");
    assert!(!taken.is_error);
    let stale_carrier = first.protected_carrier.clone().unwrap();
    let stale = attest(&mut fixture, &stale_carrier, "client-a");
    assert!(
        stale.is_error,
        "stale snapshot must not attest after lease takeover"
    );
}

#[test]
fn expired_lease_rejects_heartbeat_and_actions() {
    let mut fixture = fixture();
    let state = get_app_state(&mut fixture, "client-a");
    let carrier = state.protected_carrier.clone().unwrap();
    attest(&mut fixture, &carrier, "client-a");

    fixture.clock.advance(31.0);
    let heartbeat = fixture.runtime.call_tool(
        "lease_heartbeat",
        arguments(json!({"snapshot_id": carrier.snapshot_id})),
        &context("client-a"),
    );
    assert!(heartbeat.is_error);

    let action = fixture.runtime.call_tool(
        "click",
        pixel_click(&carrier.snapshot_id),
        &action_context("client-a", "late-action"),
    );
    assert!(action.is_error);
    assert_eq!(fixture.driver.click_count.load(Ordering::SeqCst), 0);
}

#[test]
fn heartbeat_renews_the_lease() {
    let mut fixture = fixture();
    let state = get_app_state(&mut fixture, "client-a");
    let carrier = state.protected_carrier.clone().unwrap();
    attest(&mut fixture, &carrier, "client-a");

    for _ in 0..4 {
        fixture.clock.advance(20.0);
        let heartbeat = fixture.runtime.call_tool(
            "lease_heartbeat",
            arguments(json!({"snapshot_id": carrier.snapshot_id})),
            &context("client-a"),
        );
        assert!(!heartbeat.is_error);
    }
    let clicked = fixture.runtime.call_tool(
        "click",
        pixel_click(&carrier.snapshot_id),
        &action_context("client-a", "heartbeat-action"),
    );
    assert!(!clicked.is_error);
}

#[test]
fn action_identifier_conflicts_are_rejected_and_applied_receipts_short_circuit() {
    let mut fixture = fixture();
    let state = get_app_state(&mut fixture, "client-a");
    let carrier = state.protected_carrier.clone().unwrap();
    attest(&mut fixture, &carrier, "client-a");

    let first = fixture.runtime.call_tool(
        "click",
        pixel_click(&carrier.snapshot_id),
        &action_context("client-a", "action-one"),
    );
    assert!(!first.is_error);
    assert!(first.text.contains("receipt_id=action-one"));

    // Replaying the identifier against the consumed snapshot fails closed
    // without a second dispatch (validated before the receipt short-circuit,
    // exactly like the macOS runtime).
    let replay = fixture.runtime.call_tool(
        "click",
        pixel_click(&carrier.snapshot_id),
        &action_context("client-a", "action-one"),
    );
    assert!(replay.is_error);
    assert_eq!(fixture.driver.click_count.load(Ordering::SeqCst), 1);

    // Reusing it with a different (attested) snapshot conflicts with the
    // durable receipt.
    let next_carrier = first.protected_carrier.clone().unwrap();
    attest(&mut fixture, &next_carrier, "client-a");
    let conflict = fixture.runtime.call_tool(
        "click",
        pixel_click(&next_carrier.snapshot_id),
        &action_context("client-a", "action-one"),
    );
    assert!(conflict.is_error);
    assert!(conflict.text.contains("conflicts"));
    assert_eq!(fixture.driver.click_count.load(Ordering::SeqCst), 1);
}

#[test]
fn follow_up_capture_failure_still_reports_applied() {
    let mut fixture = fixture();
    let state = get_app_state(&mut fixture, "client-a");
    let carrier = state.protected_carrier.clone().unwrap();
    attest(&mut fixture, &carrier, "client-a");

    fixture.driver.fail_capture.store(true, Ordering::SeqCst);
    let result = fixture.runtime.call_tool(
        "click",
        pixel_click(&carrier.snapshot_id),
        &action_context("client-a", "action-one"),
    );
    assert!(!result.is_error);
    assert!(result.text.contains("follow-up screenshot failed"));
    assert_eq!(
        fixture.receipts.receipt("action-one").unwrap().state,
        ActionReceiptState::Applied
    );
}

#[test]
fn startup_recovery_marks_stranded_dispatches_outcome_unknown() {
    let receipts: &'static MemoryReceiptStore = Box::leak(Box::default());
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
    ActionReceiptStore::create(
        &receipts,
        &ActionReceipt {
            identifier: "stranded".to_owned(),
            tool_name: "click".to_owned(),
            snapshot_identifier: "snap".to_owned(),
            state: ActionReceiptState::Dispatched,
            created_at: now,
            updated_at: now,
            failure_code: None,
        },
    )
    .unwrap();
    let driver: &'static FakeDesktopDriver = Box::leak(Box::new(FakeDesktopDriver::new(receipts)));
    let clock: &'static MutableClock = Box::leak(Box::new(MutableClock::new()));
    let _runtime = ComputerUseRuntime::new(
        Box::new(driver),
        Box::new(receipts),
        Box::new(clock),
        Box::new(SequenceIdentifiers {
            counter: Mutex::new(0),
        }),
        30.0,
    )
    .unwrap();
    assert_eq!(
        receipts.receipt("stranded").unwrap().state,
        ActionReceiptState::OutcomeUnknown
    );
}

#[test]
fn disconnect_releases_lease_and_snapshots() {
    let mut fixture = fixture();
    let state = get_app_state(&mut fixture, "client-a");
    let carrier = state.protected_carrier.clone().unwrap();
    fixture.runtime.client_disconnected("client-a");

    let other = get_app_state(&mut fixture, "client-b");
    assert!(!other.is_error, "lease must be free after disconnect");
    let stale = attest(&mut fixture, &carrier, "client-a");
    assert!(stale.is_error);
}

#[test]
fn unknown_arguments_fail_closed() {
    let mut fixture = fixture();
    let result = fixture.runtime.call_tool(
        "get_app_state",
        arguments(json!({"bundle_id": "com.example.fixture", "surprise": 1})),
        &context("client-a"),
    );
    assert!(result.is_error);
    assert!(result.text.contains("Unknown argument fields"));
}

#[test]
fn element_click_uses_accessibility_press() {
    let mut fixture = fixture();
    let state = get_app_state(&mut fixture, "client-a");
    let carrier = state.protected_carrier.clone().unwrap();
    attest(&mut fixture, &carrier, "client-a");
    let result = fixture.runtime.call_tool(
        "click",
        arguments(json!({
            "snapshot_id": carrier.snapshot_id,
            "target": {"kind": "element", "element_id": "e1"},
        })),
        &action_context("client-a", "element-action"),
    );
    assert!(!result.is_error, "{}", result.text);
    // The AXPress path was taken: no raw pointer click was dispatched.
    assert_eq!(fixture.driver.click_count.load(Ordering::SeqCst), 0);
}
