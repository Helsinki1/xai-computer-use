//! X11 desktop driver: EWMH window discovery, exact-window capture with the
//! v2 coordinate/PNG contract, and XTest input injection with exact-window
//! revalidation before every dispatch.
//!
//! Linux mapping decisions (documented in the README):
//! - `bundle_id` carries the WM_CLASS class name.
//! - Global coordinates are X11 root-window pixels (points == pixels).
//! - MVP observation is window-level only; AT-SPI2 element targeting is
//!   phase 2, so element-addressed driver calls fail closed.

use std::sync::Mutex;

use sha2::{Digest, Sha256};
use x11rb::connection::Connection;
use x11rb::protocol::composite::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use computer_use_core::models::{
    AppDescriptor, AppTarget, CapturedDesktopState, ComputerUseError, GlobalScreenPoint,
    GlobalScreenRect, MouseButton, Result, WindowGeometry, MAX_PNG_SIDE_PIXELS,
    MAX_PNG_TOTAL_PIXELS, MAX_PROTECTED_PNG_BYTES,
};
use computer_use_core::runtime::DesktopDriver;

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;

fn unavailable(message: &str) -> ComputerUseError {
    ComputerUseError::StateUnavailable(message.to_owned())
}

struct Atoms {
    net_client_list: xproto::Atom,
    net_wm_pid: xproto::Atom,
    net_active_window: xproto::Atom,
    net_wm_name: xproto::Atom,
    utf8_string: xproto::Atom,
}

pub struct X11Driver {
    connection: RustConnection,
    root: xproto::Window,
    atoms: Atoms,
    // Serializes multi-request input sequences and keyboard-map edits.
    input_lock: Mutex<()>,
}

struct WindowInfo {
    window: u32,
    class: Option<String>,
    pid: Option<i32>,
    title: Option<String>,
}

impl X11Driver {
    pub fn connect() -> Result<Self> {
        let (connection, screen_number) = x11rb::connect(None).map_err(|_| {
            unavailable(
                "The X11 display is unavailable. Supported MVP environment is Ubuntu on X11.",
            )
        })?;
        let root = connection.setup().roots[screen_number].root;
        let atoms = Atoms {
            net_client_list: intern(&connection, "_NET_CLIENT_LIST")?,
            net_wm_pid: intern(&connection, "_NET_WM_PID")?,
            net_active_window: intern(&connection, "_NET_ACTIVE_WINDOW")?,
            net_wm_name: intern(&connection, "_NET_WM_NAME")?,
            utf8_string: intern(&connection, "UTF8_STRING")?,
        };
        connection
            .xtest_get_version(2, 2)
            .map_err(|_| unavailable("The XTEST extension is unavailable."))?
            .reply()
            .map_err(|_| unavailable("The XTEST extension is unavailable."))?;
        Ok(Self {
            connection,
            root,
            atoms,
            input_lock: Mutex::new(()),
        })
    }

    fn client_windows(&self) -> Result<Vec<WindowInfo>> {
        let reply = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms.net_client_list,
                xproto::AtomEnum::WINDOW,
                0,
                4096,
            )
            .map_err(|_| unavailable("The window list is unavailable."))?
            .reply()
            .map_err(|_| unavailable("The window list is unavailable."))?;
        let windows: Vec<u32> = reply.value32().map(Iterator::collect).unwrap_or_default();
        Ok(windows
            .into_iter()
            .map(|window| WindowInfo {
                window,
                class: self.window_class(window),
                pid: self.window_pid(window),
                title: self.window_title(window),
            })
            .collect())
    }

    fn window_class(&self, window: u32) -> Option<String> {
        let reply = self
            .connection
            .get_property(
                false,
                window,
                xproto::AtomEnum::WM_CLASS,
                xproto::AtomEnum::STRING,
                0,
                256,
            )
            .ok()?
            .reply()
            .ok()?;
        let value = reply.value;
        // WM_CLASS is "instance\0class\0"; the class name is the stable
        // application identity.
        let mut parts = value
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty());
        let instance = parts.next();
        let class = parts.next().or(instance)?;
        String::from_utf8(class.to_vec()).ok()
    }

    fn window_pid(&self, window: u32) -> Option<i32> {
        let reply = self
            .connection
            .get_property(
                false,
                window,
                self.atoms.net_wm_pid,
                xproto::AtomEnum::CARDINAL,
                0,
                1,
            )
            .ok()?
            .reply()
            .ok()?;
        let pid = reply.value32().and_then(|mut values| values.next());
        i32::try_from(pid?).ok()
    }

    fn window_title(&self, window: u32) -> Option<String> {
        let utf8 = self
            .connection
            .get_property(
                false,
                window,
                self.atoms.net_wm_name,
                self.atoms.utf8_string,
                0,
                1024,
            )
            .ok()?
            .reply()
            .ok()?;
        if utf8.value_len > 0 {
            return String::from_utf8(utf8.value).ok();
        }
        let legacy = self
            .connection
            .get_property(
                false,
                window,
                xproto::AtomEnum::WM_NAME,
                xproto::AtomEnum::STRING,
                0,
                1024,
            )
            .ok()?
            .reply()
            .ok()?;
        (legacy.value_len > 0).then(|| String::from_utf8_lossy(&legacy.value).into_owned())
    }

    fn active_window(&self) -> Option<u32> {
        let reply = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms.net_active_window,
                xproto::AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()?
            .reply()
            .ok()?;
        let window = reply.value32().and_then(|mut values| values.next());
        window
    }

    fn global_bounds(&self, window: u32) -> Result<GlobalScreenRect> {
        let geometry = self
            .connection
            .get_geometry(window)
            .map_err(|_| unavailable("The window geometry is unavailable."))?
            .reply()
            .map_err(|_| unavailable("The window geometry is unavailable."))?;
        let translated = self
            .connection
            .translate_coordinates(window, self.root, 0, 0)
            .map_err(|_| unavailable("The window geometry is unavailable."))?
            .reply()
            .map_err(|_| unavailable("The window geometry is unavailable."))?;
        Ok(GlobalScreenRect {
            x: f64::from(translated.dst_x),
            y: f64::from(translated.dst_y),
            width: f64::from(geometry.width),
            height: f64::from(geometry.height),
        })
    }

    /// Revalidates the exact captured window immediately before an input
    /// dispatch: same window, same process, same global geometry, viewable.
    fn revalidate(&self, app: &AppTarget, expected: &WindowGeometry) -> Result<()> {
        let window = expected.window_identifier;
        let attributes = self
            .connection
            .get_window_attributes(window)
            .map_err(|_| unavailable("The captured window no longer exists."))?
            .reply()
            .map_err(|_| unavailable("The captured window no longer exists."))?;
        if attributes.map_state != xproto::MapState::VIEWABLE {
            return Err(unavailable("The captured window is no longer viewable."));
        }
        if self.window_pid(window) != Some(app.process_identifier) {
            return Err(unavailable("The captured window changed ownership."));
        }
        let bounds = self.global_bounds(window)?;
        if bounds != expected.global_bounds_points {
            return Err(unavailable(
                "The captured window moved or resized after the snapshot.",
            ));
        }
        Ok(())
    }

    fn resolve_window(
        &self,
        windows: Vec<WindowInfo>,
        requested: Option<u32>,
    ) -> Result<WindowInfo> {
        if windows.is_empty() {
            return Err(unavailable("No window matches the requested application."));
        }
        if let Some(requested) = requested {
            return windows
                .into_iter()
                .find(|info| info.window == requested)
                .ok_or_else(|| {
                    unavailable("The requested window does not belong to the application.")
                });
        }
        let active = self.active_window();
        let mut windows = windows;
        if let Some(active) = active {
            if let Some(index) = windows.iter().position(|info| info.window == active) {
                return Ok(windows.swap_remove(index));
            }
        }
        Ok(windows.swap_remove(0))
    }

    fn capture_window(&self, info: &WindowInfo) -> Result<CapturedDesktopState> {
        let window = info.window;
        let pid = info
            .pid
            .ok_or_else(|| unavailable("The window does not advertise _NET_WM_PID."))?;
        let class = info
            .class
            .clone()
            .ok_or_else(|| unavailable("The window does not advertise WM_CLASS."))?;
        let bounds = self.global_bounds(window)?;
        let width = bounds.width as u16;
        let height = bounds.height as u16;
        if width == 0 || height == 0 {
            return Err(unavailable("The window has no capturable area."));
        }
        let raw = self.window_image_rgba(window, width, height)?;
        let (png, png_width, png_height) =
            encode_bounded_png(&raw, u32::from(width), u32::from(height))?;
        let mut hasher = Sha256::new();
        hasher.update(&png);
        let sha256 = hex(&hasher.finalize());
        let app = AppTarget {
            name: class.clone(),
            bundle_identifier: Some(class.clone()),
            process_identifier: pid,
        };
        let title = info.title.clone();
        let tree = format!(
            "window_id={window} pid={pid} class={class} title=\"{}\"\nNo accessibility elements are available in this build; use pixel targets. (AT-SPI2 semantic targeting is phase 2.)",
            title.as_deref().unwrap_or("")
        );
        Ok(CapturedDesktopState {
            app,
            window_title: title,
            geometry: WindowGeometry {
                window_identifier: window,
                global_bounds_points: bounds,
                png_width_pixels: png_width,
                png_height_pixels: png_height,
            },
            screenshot_png: png,
            screenshot_sha256: sha256,
            accessibility_tree: tree,
            elements: Vec::new(),
        })
    }

    /// Fetches the exact window contents as RGBA8. Prefers the composite
    /// backing pixmap (exact even when partially obscured); falls back to a
    /// direct window GetImage.
    fn window_image_rgba(&self, window: u32, width: u16, height: u16) -> Result<Vec<u8>> {
        let via_composite =
            (|| -> std::result::Result<xproto::GetImageReply, Box<dyn std::error::Error>> {
                self.connection.composite_query_version(0, 4)?.reply()?;
                self.connection
                    .composite_redirect_window(window, composite::Redirect::AUTOMATIC)?
                    .check()?;
                let pixmap = self.connection.generate_id()?;
                self.connection
                    .composite_name_window_pixmap(window, pixmap)?
                    .check()?;
                let image = self
                    .connection
                    .get_image(
                        xproto::ImageFormat::Z_PIXMAP,
                        pixmap,
                        0,
                        0,
                        width,
                        height,
                        !0,
                    )?
                    .reply();
                let _ = self.connection.free_pixmap(pixmap);
                Ok(image?)
            })();
        let reply = match via_composite {
            Ok(reply) => reply,
            Err(_) => self
                .connection
                .get_image(
                    xproto::ImageFormat::Z_PIXMAP,
                    window,
                    0,
                    0,
                    width,
                    height,
                    !0,
                )
                .map_err(|_| unavailable("The window contents could not be captured."))?
                .reply()
                .map_err(|_| unavailable("The window contents could not be captured."))?,
        };
        zpixmap_to_rgba(
            &self.connection,
            &reply,
            u32::from(width),
            u32::from(height),
        )
    }

    fn fake_input(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<()> {
        self.connection
            .xtest_fake_input(kind, detail, x11rb::CURRENT_TIME, self.root, x, y, 0)
            .map_err(|_| unavailable("Input injection failed."))?
            .check()
            .map_err(|_| unavailable("Input injection failed."))
    }

    fn move_pointer(&self, point: GlobalScreenPoint) -> Result<()> {
        let (x, y) = device_point(point)?;
        self.fake_input(MOTION_NOTIFY, 0, x, y)
    }

    fn press_button(&self, button: u8) -> Result<()> {
        self.fake_input(BUTTON_PRESS, button, 0, 0)
    }

    fn release_button(&self, button: u8) -> Result<()> {
        self.fake_input(BUTTON_RELEASE, button, 0, 0)
    }

    fn focus_window(&self, window: u32) -> Result<()> {
        self.connection
            .set_input_focus(xproto::InputFocus::PARENT, window, x11rb::CURRENT_TIME)
            .map_err(|_| unavailable("The window could not be focused."))?
            .check()
            .map_err(|_| unavailable("The window could not be focused."))
    }

    fn keyboard(&self) -> Result<KeyboardMap> {
        KeyboardMap::fetch(&self.connection)
    }

    fn tap_keycode(&self, keycode: u8, shift: bool, keyboard: &KeyboardMap) -> Result<()> {
        if shift {
            self.fake_input(KEY_PRESS, keyboard.shift_keycode, 0, 0)?;
        }
        self.fake_input(KEY_PRESS, keycode, 0, 0)?;
        self.fake_input(KEY_RELEASE, keycode, 0, 0)?;
        if shift {
            self.fake_input(KEY_RELEASE, keyboard.shift_keycode, 0, 0)?;
        }
        Ok(())
    }
}

fn intern(connection: &RustConnection, name: &str) -> Result<xproto::Atom> {
    Ok(connection
        .intern_atom(false, name.as_bytes())
        .map_err(|_| unavailable("The X11 connection failed."))?
        .reply()
        .map_err(|_| unavailable("The X11 connection failed."))?
        .atom)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Converts continuous global coordinates to X11 device pixels. The exact
/// mapping contract has no rounding; the integer device grid is entered here,
/// at the last possible moment, by flooring into the containing pixel cell.
fn device_point(point: GlobalScreenPoint) -> Result<(i16, i16)> {
    if !point.x.is_finite() || !point.y.is_finite() {
        return Err(unavailable("The action point is not finite."));
    }
    let x = point.x.floor();
    let y = point.y.floor();
    if x < f64::from(i16::MIN)
        || x > f64::from(i16::MAX)
        || y < f64::from(i16::MIN)
        || y > f64::from(i16::MAX)
    {
        return Err(unavailable("The action point is outside the device space."));
    }
    Ok((x as i16, y as i16))
}

fn zpixmap_to_rgba(
    connection: &RustConnection,
    reply: &xproto::GetImageReply,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let setup = connection.setup();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|format| u32::from(format.depth) == reply.depth as u32)
        .ok_or_else(|| unavailable("The captured image format is unsupported."))?;
    let bits_per_pixel = format.bits_per_pixel;
    if !(reply.depth == 24 || reply.depth == 32) || bits_per_pixel != 32 {
        return Err(unavailable("The captured image format is unsupported."));
    }
    let lsb_first = setup.image_byte_order == xproto::ImageOrder::LSB_FIRST;
    let expected = (width as usize) * (height as usize) * 4;
    if reply.data.len() < expected {
        return Err(unavailable("The captured image is incomplete."));
    }
    let mut rgba = vec![0u8; expected];
    for (pixel, chunk) in reply.data[..expected].chunks_exact(4).enumerate() {
        let value = if lsb_first {
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        } else {
            u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        };
        // Standard TrueColor layout: 0x00RRGGBB.
        let offset = pixel * 4;
        rgba[offset] = ((value >> 16) & 0xff) as u8;
        rgba[offset + 1] = ((value >> 8) & 0xff) as u8;
        rgba[offset + 2] = (value & 0xff) as u8;
        rgba[offset + 3] = 0xff;
    }
    Ok(rgba)
}

/// Downscales (box filter) and encodes RGBA8 into a PNG satisfying the v2
/// bounds: <= 1,280 px per side, <= 1,638,400 px total, <= 900,000 bytes.
/// Fails closed when the contract cannot be met.
fn encode_bounded_png(rgba: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, u32, u32)> {
    let mut scale = 1.0f64;
    scale = scale.min(f64::from(MAX_PNG_SIDE_PIXELS) / f64::from(width));
    scale = scale.min(f64::from(MAX_PNG_SIDE_PIXELS) / f64::from(height));
    let total = u64::from(width) * u64::from(height);
    if total > MAX_PNG_TOTAL_PIXELS {
        scale = scale.min((MAX_PNG_TOTAL_PIXELS as f64 / total as f64).sqrt());
    }
    for _ in 0..8 {
        let target_width = ((f64::from(width) * scale).floor() as u32)
            .max(1)
            .min(width);
        let target_height = ((f64::from(height) * scale).floor() as u32)
            .max(1)
            .min(height);
        let scaled = if target_width == width && target_height == height {
            rgba.to_vec()
        } else {
            box_downscale(rgba, width, height, target_width, target_height)
        };
        let png = encode_png(&scaled, target_width, target_height)?;
        if png.len() <= MAX_PROTECTED_PNG_BYTES {
            return Ok((png, target_width, target_height));
        }
        scale *= 0.8;
    }
    Err(unavailable(
        "The screenshot could not satisfy the bounded PNG contract.",
    ))
}

fn box_downscale(
    rgba: &[u8],
    width: u32,
    height: u32,
    target_width: u32,
    target_height: u32,
) -> Vec<u8> {
    let mut result = vec![0u8; target_width as usize * target_height as usize * 4];
    for ty in 0..target_height {
        let y0 = (u64::from(ty) * u64::from(height) / u64::from(target_height)) as u32;
        let y1 = (((u64::from(ty) + 1) * u64::from(height)).div_ceil(u64::from(target_height))
            as u32)
            .clamp(y0 + 1, height);
        for tx in 0..target_width {
            let x0 = (u64::from(tx) * u64::from(width) / u64::from(target_width)) as u32;
            let x1 = (((u64::from(tx) + 1) * u64::from(width)).div_ceil(u64::from(target_width))
                as u32)
                .clamp(x0 + 1, width);
            let mut sums = [0u64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let offset = (y as usize * width as usize + x as usize) * 4;
                    sums[0] += u64::from(rgba[offset]);
                    sums[1] += u64::from(rgba[offset + 1]);
                    sums[2] += u64::from(rgba[offset + 2]);
                }
            }
            let count = u64::from(y1 - y0) * u64::from(x1 - x0);
            let offset = (ty as usize * target_width as usize + tx as usize) * 4;
            result[offset] = (sums[0] / count) as u8;
            result[offset + 1] = (sums[1] / count) as u8;
            result[offset + 2] = (sums[2] / count) as u8;
            result[offset + 3] = 0xff;
        }
    }
    result
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut buffer, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut writer = encoder
            .write_header()
            .map_err(|_| unavailable("The screenshot could not be encoded."))?;
        writer
            .write_image_data(rgba)
            .map_err(|_| unavailable("The screenshot could not be encoded."))?;
    }
    Ok(buffer)
}

struct KeyboardMap {
    min_keycode: u8,
    keysyms_per_keycode: u8,
    keysyms: Vec<u32>,
    shift_keycode: u8,
}

const KEYSYM_SHIFT_L: u32 = 0xffe1;
const KEYSYM_CONTROL_L: u32 = 0xffe3;
const KEYSYM_ALT_L: u32 = 0xffe9;
const KEYSYM_SUPER_L: u32 = 0xffeb;

impl KeyboardMap {
    fn fetch(connection: &RustConnection) -> Result<Self> {
        let setup = connection.setup();
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let reply = connection
            .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)
            .map_err(|_| unavailable("The keyboard mapping is unavailable."))?
            .reply()
            .map_err(|_| unavailable("The keyboard mapping is unavailable."))?;
        let mut map = Self {
            min_keycode,
            keysyms_per_keycode: reply.keysyms_per_keycode,
            keysyms: reply.keysyms,
            shift_keycode: 0,
        };
        map.shift_keycode = map
            .find(KEYSYM_SHIFT_L)
            .map(|(keycode, _)| keycode)
            .ok_or_else(|| unavailable("The keyboard has no Shift key."))?;
        Ok(map)
    }

    /// Finds a keycode producing `keysym`; the boolean is whether Shift is
    /// required (level 1 instead of level 0).
    fn find(&self, keysym: u32) -> Option<(u8, bool)> {
        let per = usize::from(self.keysyms_per_keycode.max(1));
        for (index, group) in self.keysyms.chunks(per).enumerate() {
            let keycode = self.min_keycode as usize + index;
            if group.first() == Some(&keysym) {
                return Some((keycode as u8, false));
            }
            if per > 1 && group.get(1) == Some(&keysym) {
                return Some((keycode as u8, true));
            }
        }
        None
    }

    /// A keycode with no bound keysyms, usable for temporary remapping.
    fn spare_keycode(&self) -> Option<u8> {
        let per = usize::from(self.keysyms_per_keycode.max(1));
        self.keysyms
            .chunks(per)
            .enumerate()
            .rev()
            .find(|(_, group)| group.iter().all(|keysym| *keysym == 0))
            .map(|(index, _)| (self.min_keycode as usize + index) as u8)
    }
}

fn char_keysym(character: char) -> u32 {
    let code = character as u32;
    match code {
        // Latin-1 maps directly onto keysym space.
        0x20..=0x7e | 0xa0..=0xff => code,
        0x09 => 0xff09,        // Tab
        0x0a | 0x0d => 0xff0d, // Return
        _ => 0x0100_0000 + code,
    }
}

fn named_keysym(name: &str) -> Option<u32> {
    let lowered = name.to_lowercase();
    Some(match lowered.as_str() {
        "return" | "enter" => 0xff0d,
        "tab" => 0xff09,
        "space" => 0x20,
        "escape" | "esc" => 0xff1b,
        // macOS "delete" is backward delete.
        "delete" | "backspace" => 0xff08,
        "forwarddelete" => 0xffff,
        "left" | "arrowleft" => 0xff51,
        "up" | "arrowup" => 0xff52,
        "right" | "arrowright" => 0xff53,
        "down" | "arrowdown" => 0xff54,
        "home" => 0xff50,
        "end" => 0xff57,
        "pageup" => 0xff55,
        "pagedown" => 0xff56,
        "f1" => 0xffbe,
        "f2" => 0xffbf,
        "f3" => 0xffc0,
        "f4" => 0xffc1,
        "f5" => 0xffc2,
        "f6" => 0xffc3,
        "f7" => 0xffc4,
        "f8" => 0xffc5,
        "f9" => 0xffc6,
        "f10" => 0xffc7,
        "f11" => 0xffc8,
        "f12" => 0xffc9,
        _ => {
            let mut characters = name.chars();
            let (Some(character), None) = (characters.next(), characters.next()) else {
                return None;
            };
            char_keysym(character)
        }
    })
}

fn modifier_keysym(name: &str) -> Result<u32> {
    match name {
        // macOS modifier vocabulary mapped onto X11 modifiers.
        "command" | "cmd" | "super" => Ok(KEYSYM_SUPER_L),
        "control" | "ctrl" => Ok(KEYSYM_CONTROL_L),
        "option" | "alt" => Ok(KEYSYM_ALT_L),
        "shift" => Ok(KEYSYM_SHIFT_L),
        "fn" => Err(ComputerUseError::InvalidArguments(
            "The fn modifier has no X11 equivalent.".to_owned(),
        )),
        _ => Err(ComputerUseError::InvalidArguments(
            "modifiers is invalid.".to_owned(),
        )),
    }
}

impl DesktopDriver for X11Driver {
    fn list_apps(&self) -> Result<Vec<AppDescriptor>> {
        let windows = self.client_windows()?;
        let active = self.active_window();
        let mut apps: Vec<AppDescriptor> = Vec::new();
        for info in windows {
            let (Some(class), Some(pid)) = (info.class.clone(), info.pid) else {
                continue;
            };
            let is_active = active == Some(info.window);
            if let Some(existing) = apps.iter_mut().find(|app| {
                app.bundle_identifier.as_deref() == Some(class.as_str())
                    && app.process_identifier == pid
            }) {
                existing.window_identifiers.push(info.window);
                existing.is_active |= is_active;
            } else {
                apps.push(AppDescriptor {
                    name: class.clone(),
                    bundle_identifier: Some(class),
                    process_identifier: pid,
                    is_active,
                    window_identifiers: vec![info.window],
                });
            }
        }
        Ok(apps)
    }

    fn capture_by_bundle(
        &self,
        bundle_identifier: &str,
        window_identifier: Option<u32>,
    ) -> Result<CapturedDesktopState> {
        let windows = self
            .client_windows()?
            .into_iter()
            .filter(|info| info.class.as_deref() == Some(bundle_identifier))
            .collect();
        let window = self.resolve_window(windows, window_identifier)?;
        self.capture_window(&window)
    }

    fn capture_by_process(
        &self,
        process_identifier: i32,
        window_identifier: Option<u32>,
    ) -> Result<CapturedDesktopState> {
        let windows = self
            .client_windows()?
            .into_iter()
            .filter(|info| info.pid == Some(process_identifier))
            .collect();
        let window = self.resolve_window(windows, window_identifier)?;
        self.capture_window(&window)
    }

    fn click(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        point: GlobalScreenPoint,
        button: MouseButton,
        count: u32,
    ) -> Result<()> {
        let _guard = self.input_lock.lock().expect("input lock");
        self.revalidate(app, expected_geometry)?;
        let device_button = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
        };
        self.move_pointer(point)?;
        for _ in 0..count.clamp(1, 2) {
            self.press_button(device_button)?;
            self.release_button(device_button)?;
        }
        self.connection
            .flush()
            .map_err(|_| unavailable("Input injection failed."))
    }

    fn perform_accessibility_action(
        &self,
        _app: &AppTarget,
        _expected_geometry: &WindowGeometry,
        _driver_token: &str,
        _action: &str,
    ) -> Result<()> {
        Err(ComputerUseError::InvalidArguments(
            "No accessibility elements are available in this snapshot. (AT-SPI2 is phase 2.)"
                .to_owned(),
        ))
    }

    fn scroll(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        point: GlobalScreenPoint,
        delta_x: f64,
        delta_y: f64,
    ) -> Result<()> {
        let _guard = self.input_lock.lock().expect("input lock");
        self.revalidate(app, expected_geometry)?;
        self.move_pointer(point)?;
        // Deltas are in macOS line units; one X11 wheel detent is ~3 lines.
        let vertical_button = if delta_y > 0.0 { 4 } else { 5 };
        let horizontal_button = if delta_x > 0.0 { 6 } else { 7 };
        let vertical_clicks =
            (delta_y.abs() / 3.0)
                .round()
                .max(if delta_y == 0.0 { 0.0 } else { 1.0 }) as u32;
        let horizontal_clicks =
            (delta_x.abs() / 3.0)
                .round()
                .max(if delta_x == 0.0 { 0.0 } else { 1.0 }) as u32;
        for _ in 0..vertical_clicks.min(64) {
            self.press_button(vertical_button)?;
            self.release_button(vertical_button)?;
        }
        for _ in 0..horizontal_clicks.min(64) {
            self.press_button(horizontal_button)?;
            self.release_button(horizontal_button)?;
        }
        self.connection
            .flush()
            .map_err(|_| unavailable("Input injection failed."))
    }

    fn drag(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        from: GlobalScreenPoint,
        to: GlobalScreenPoint,
    ) -> Result<()> {
        let _guard = self.input_lock.lock().expect("input lock");
        self.revalidate(app, expected_geometry)?;
        self.move_pointer(from)?;
        self.press_button(1)?;
        const STEPS: u32 = 12;
        for step in 1..=STEPS {
            let fraction = f64::from(step) / f64::from(STEPS);
            self.move_pointer(GlobalScreenPoint {
                x: from.x + (to.x - from.x) * fraction,
                y: from.y + (to.y - from.y) * fraction,
            })?;
            self.connection
                .flush()
                .map_err(|_| unavailable("Input injection failed."))?;
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        self.release_button(1)?;
        self.connection
            .flush()
            .map_err(|_| unavailable("Input injection failed."))
    }

    fn type_text(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        text: &str,
    ) -> Result<()> {
        let _guard = self.input_lock.lock().expect("input lock");
        self.revalidate(app, expected_geometry)?;
        self.focus_window(expected_geometry.window_identifier)?;
        let keyboard = self.keyboard()?;
        let spare = keyboard.spare_keycode();
        let mut remapped: Option<(u8, u32)> = None;
        let outcome = (|| -> Result<()> {
            for character in text.chars() {
                let keysym = char_keysym(character);
                if let Some((keycode, shift)) = keyboard.find(keysym) {
                    self.tap_keycode(keycode, shift, &keyboard)?;
                    continue;
                }
                let spare = spare.ok_or_else(|| {
                    ComputerUseError::InvalidArguments(
                        "The text contains characters the keyboard map cannot produce.".to_owned(),
                    )
                })?;
                if remapped.map(|(_, current)| current) != Some(keysym) {
                    let per = usize::from(keyboard.keysyms_per_keycode.max(1));
                    let mut keysyms = vec![0u32; per];
                    keysyms[0] = keysym;
                    self.connection
                        .change_keyboard_mapping(1, spare, keyboard.keysyms_per_keycode, &keysyms)
                        .map_err(|_| unavailable("Input injection failed."))?
                        .check()
                        .map_err(|_| unavailable("Input injection failed."))?;
                    self.connection
                        .flush()
                        .map_err(|_| unavailable("Input injection failed."))?;
                    remapped = Some((spare, keysym));
                }
                self.tap_keycode(spare, false, &keyboard)?;
                self.connection
                    .flush()
                    .map_err(|_| unavailable("Input injection failed."))?;
                // Give clients a beat to observe the remapped symbol before
                // it changes again.
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Ok(())
        })();
        if let Some((spare, _)) = remapped {
            let per = usize::from(keyboard.keysyms_per_keycode.max(1));
            let zeros = vec![0u32; per];
            let _ = self
                .connection
                .change_keyboard_mapping(1, spare, keyboard.keysyms_per_keycode, &zeros)
                .map(|cookie| cookie.check());
        }
        self.connection
            .flush()
            .map_err(|_| unavailable("Input injection failed."))?;
        outcome
    }

    fn press_key(
        &self,
        app: &AppTarget,
        expected_geometry: &WindowGeometry,
        specification: &str,
    ) -> Result<()> {
        let _guard = self.input_lock.lock().expect("input lock");
        self.revalidate(app, expected_geometry)?;
        self.focus_window(expected_geometry.window_identifier)?;
        let keyboard = self.keyboard()?;
        let mut parts: Vec<&str> = specification.split('+').collect();
        let key_name = parts
            .pop()
            .ok_or_else(|| ComputerUseError::InvalidArguments("key is invalid.".to_owned()))?;
        let mut modifier_keycodes = Vec::new();
        for part in parts {
            let keysym = modifier_keysym(&part.to_lowercase())?;
            let (keycode, _) = keyboard.find(keysym).ok_or_else(|| {
                unavailable("The keyboard map does not provide the requested modifier.")
            })?;
            modifier_keycodes.push(keycode);
        }
        let keysym = named_keysym(key_name)
            .ok_or_else(|| ComputerUseError::InvalidArguments("key is invalid.".to_owned()))?;
        let (keycode, shift) = keyboard
            .find(keysym)
            .ok_or_else(|| unavailable("The keyboard map cannot produce the requested key."))?;
        for modifier in &modifier_keycodes {
            self.fake_input(KEY_PRESS, *modifier, 0, 0)?;
        }
        self.tap_keycode(keycode, shift, &keyboard)?;
        for modifier in modifier_keycodes.iter().rev() {
            self.fake_input(KEY_RELEASE, *modifier, 0, 0)?;
        }
        self.connection
            .flush()
            .map_err(|_| unavailable("Input injection failed."))
    }

    fn set_value(
        &self,
        _app: &AppTarget,
        _expected_geometry: &WindowGeometry,
        _driver_token: &str,
        _value: &str,
    ) -> Result<()> {
        Err(ComputerUseError::InvalidArguments(
            "No accessibility elements are available in this snapshot. (AT-SPI2 is phase 2.)"
                .to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_png_respects_all_three_limits() {
        // A 2000x1500 gradient must downscale to <=1280/side and <=1,638,400 px.
        let width = 2000u32;
        let height = 1500u32;
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let offset = ((y * width + x) * 4) as usize;
                rgba[offset] = (x % 256) as u8;
                rgba[offset + 1] = (y % 256) as u8;
                rgba[offset + 2] = ((x + y) % 256) as u8;
                rgba[offset + 3] = 0xff;
            }
        }
        let (png, out_width, out_height) = encode_bounded_png(&rgba, width, height).unwrap();
        assert!(out_width <= MAX_PNG_SIDE_PIXELS);
        assert!(out_height <= MAX_PNG_SIDE_PIXELS);
        assert!(u64::from(out_width) * u64::from(out_height) <= MAX_PNG_TOTAL_PIXELS);
        assert!(png.len() <= MAX_PROTECTED_PNG_BYTES);
        // Aspect ratio is preserved within a pixel.
        let ratio_in = f64::from(width) / f64::from(height);
        let ratio_out = f64::from(out_width) / f64::from(out_height);
        assert!((ratio_in - ratio_out).abs() < 0.01);
    }

    #[test]
    fn small_images_pass_through_unscaled() {
        let (png, width, height) = encode_bounded_png(&[0u8; 4 * 100], 10, 10).unwrap();
        assert_eq!((width, height), (10, 10));
        assert!(!png.is_empty());
    }

    #[test]
    fn keysym_mapping_covers_ascii_named_keys_and_unicode() {
        assert_eq!(char_keysym('a'), 0x61);
        assert_eq!(char_keysym('\n'), 0xff0d);
        assert_eq!(char_keysym('é'), 0xe9);
        assert_eq!(char_keysym('€'), 0x0100_0000 + 0x20ac);
        assert_eq!(named_keysym("Return"), Some(0xff0d));
        assert_eq!(named_keysym("a"), Some(0x61));
        assert_eq!(named_keysym("not-a-key"), None);
        assert!(modifier_keysym("command").is_ok());
        assert!(modifier_keysym("fn").is_err());
    }

    #[test]
    fn device_points_floor_into_pixel_cells() {
        assert_eq!(
            device_point(GlobalScreenPoint { x: 10.9, y: 20.0 }).unwrap(),
            (10, 20)
        );
        assert!(device_point(GlobalScreenPoint {
            x: f64::NAN,
            y: 0.0
        })
        .is_err());
        assert!(device_point(GlobalScreenPoint { x: 1e9, y: 0.0 }).is_err());
    }
}
