use crate::p2p::{DesktopInput, MouseButton};
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct PendingMouseMove {
	dx: i32,
	dy: i32,
}

static MOUSE_MOVE_WORKER_ACTIVE: AtomicBool = AtomicBool::new(false);
static PENDING_MOUSE_MOVE: OnceLock<Mutex<Option<PendingMouseMove>>> = OnceLock::new();

pub(crate) async fn apply(input: DesktopInput) -> Result<()> {
	match input {
		DesktopInput::MouseMove { dx, dy } => {
			submit_mouse_move(dx, dy);
			Ok(())
		}
		input => tokio::task::spawn_blocking(move || apply_blocking(input))
			.await
			.context("desktop input task failed")?,
	}
}

fn apply_blocking(input: DesktopInput) -> Result<()> {
	match input {
		DesktopInput::MouseMove { dx, dy } => {
			submit_mouse_move(dx, dy);
			Ok(())
		}
		DesktopInput::MouseScroll { amount } => scroll_mouse(amount),
		DesktopInput::MouseClick { button } => click_mouse(button),
		DesktopInput::MousePress { button } => press_mouse(button),
		DesktopInput::MouseRelease { button } => release_mouse(button),
		DesktopInput::KeyboardText { text } => type_text(&text),
		DesktopInput::KeyboardKey { key } => press_key(&key),
	}
}

fn pending_mouse_move() -> &'static Mutex<Option<PendingMouseMove>> {
	PENDING_MOUSE_MOVE.get_or_init(|| Mutex::new(None))
}

fn lock_pending_mouse_move() -> std::sync::MutexGuard<'static, Option<PendingMouseMove>> {
	pending_mouse_move()
		.lock()
		.unwrap_or_else(|err| err.into_inner())
}

fn take_pending_mouse_move() -> Option<PendingMouseMove> {
	lock_pending_mouse_move().take()
}

fn submit_mouse_move(dx: i32, dy: i32) {
	if dx == 0 && dy == 0 {
		return;
	}
	*lock_pending_mouse_move() = Some(PendingMouseMove { dx, dy });
	if !MOUSE_MOVE_WORKER_ACTIVE.swap(true, Ordering::AcqRel) {
		std::thread::spawn(mouse_move_worker);
	}
}

fn mouse_move_worker() {
	loop {
		let Some(next) = take_pending_mouse_move() else {
			MOUSE_MOVE_WORKER_ACTIVE.store(false, Ordering::Release);
			if lock_pending_mouse_move().is_none() {
				break;
			}
			if MOUSE_MOVE_WORKER_ACTIVE.swap(true, Ordering::AcqRel) {
				break;
			}
			continue;
		};
		if let Err(err) = run_mouse_move(next.dx, next.dy) {
			log::warn!("failed to move mouse: {err:#}");
		}
	}
}

fn wait_for_mouse_moves() {
	let started = Instant::now();
	while MOUSE_MOVE_WORKER_ACTIVE.load(Ordering::Acquire) || lock_pending_mouse_move().is_some() {
		if started.elapsed() > Duration::from_millis(250) {
			break;
		}
		std::thread::sleep(Duration::from_millis(1));
	}
}

fn xdotool_mouse_button_arg(button: MouseButton) -> &'static str {
	match button {
		MouseButton::Left => "1",
		MouseButton::Middle => "2",
		MouseButton::Right => "3",
	}
}

fn ydotool_mouse_button_arg(button: MouseButton) -> &'static str {
	match button {
		MouseButton::Left => "1",
		MouseButton::Right => "2",
		MouseButton::Middle => "3",
	}
}

fn ydotool_key_sequence(key: &str) -> Option<[&'static str; 2]> {
	match key {
		"Return" => Some(["28:1", "28:0"]),
		"Tab" => Some(["15:1", "15:0"]),
		"BackSpace" => Some(["14:1", "14:0"]),
		"Escape" => Some(["1:1", "1:0"]),
		"Up" => Some(["103:1", "103:0"]),
		"Down" => Some(["108:1", "108:0"]),
		"Left" => Some(["105:1", "105:0"]),
		"Right" => Some(["106:1", "106:0"]),
		_ => None,
	}
}

fn run_mouse_move(dx: i32, dy: i32) -> Result<()> {
	match uinput_mouse::move_relative(dx, dy) {
		Ok(()) => Ok(()),
		Err(err) => {
			log::warn!("uinput mouse move failed, falling back to xdotool: {err:#}");
			run_xdotool(["mousemove_relative", "--", &dx.to_string(), &dy.to_string()])
		}
	}
}

fn scroll_mouse(amount: i32) -> Result<()> {
	match uinput_mouse::scroll(amount) {
		Ok(()) => return Ok(()),
		Err(err) => log::warn!("uinput mouse scroll failed, falling back to xdotool: {err:#}"),
	}
	let button = if amount < 0 { "4" } else { "5" };
	for _ in 0..amount.unsigned_abs().min(20) {
		run_xdotool(["click", button])?;
	}
	Ok(())
}

fn click_mouse(button: MouseButton) -> Result<()> {
	wait_for_mouse_moves();
	match uinput_mouse::click(button) {
		Ok(()) => return Ok(()),
		Err(err) => log::warn!("uinput mouse click failed, falling back to ydotool: {err:#}"),
	}
	match run_ydotool(["click", ydotool_mouse_button_arg(button)]) {
		Ok(()) => Ok(()),
		Err(err) if is_command_not_found(&err, "ydotool") => {
			run_xdotool(["click", xdotool_mouse_button_arg(button)])
		}
		Err(err) => Err(err),
	}
}

fn press_mouse(button: MouseButton) -> Result<()> {
	wait_for_mouse_moves();
	match uinput_mouse::press(button) {
		Ok(()) => Ok(()),
		Err(err) => {
			log::warn!("uinput mouse press failed, falling back to xdotool: {err:#}");
			run_xdotool(["mousedown", xdotool_mouse_button_arg(button)])
		}
	}
}

fn release_mouse(button: MouseButton) -> Result<()> {
	wait_for_mouse_moves();
	match uinput_mouse::release(button) {
		Ok(()) => Ok(()),
		Err(err) => {
			log::warn!("uinput mouse release failed, falling back to xdotool: {err:#}");
			run_xdotool(["mouseup", xdotool_mouse_button_arg(button)])
		}
	}
}

fn type_text(text: &str) -> Result<()> {
	if text.is_empty() {
		return Ok(());
	}
	match uinput_mouse::type_text(text) {
		Ok(()) => return Ok(()),
		Err(err) => log::warn!("uinput keyboard type failed, falling back to ydotool: {err:#}"),
	}
	match run_ydotool_stdin(
		["type", "--delay", "0", "--key-delay", "0", "--file", "-"],
		text,
	) {
		Ok(()) => Ok(()),
		Err(err) if is_command_not_found(&err, "ydotool") => {
			run_xdotool(["type", "--clearmodifiers", "--delay", "0", "--", text])
		}
		Err(err) => Err(err),
	}
}

fn press_key(key: &str) -> Result<()> {
	let key = key.trim();
	if key.is_empty() {
		return Ok(());
	}
	match uinput_mouse::press_key(key) {
		Ok(()) => return Ok(()),
		Err(err) => log::warn!("uinput keyboard key failed, falling back to ydotool: {err:#}"),
	}
	match ydotool_key_sequence(key) {
		Some([down, up]) => match run_ydotool(["key", down, up]) {
			Ok(()) => Ok(()),
			Err(err) if is_command_not_found(&err, "ydotool") => {
				run_xdotool(["key", "--clearmodifiers", key])
			}
			Err(err) => Err(err),
		},
		None => run_xdotool(["key", "--clearmodifiers", key]),
	}
}

fn is_command_not_found(err: &anyhow::Error, command: &str) -> bool {
	err.chain().any(|cause| {
		cause
			.downcast_ref::<std::io::Error>()
			.map(|err| err.kind() == std::io::ErrorKind::NotFound)
			.unwrap_or(false)
			&& cause.to_string().contains(command)
	})
}

fn run_xdotool<const N: usize>(args: [&str; N]) -> Result<()> {
	let output = command_output("xdotool", args)?;
	if output.status.success() {
		Ok(())
	} else {
		let stderr = String::from_utf8_lossy(&output.stderr);
		bail!("xdotool failed: {}", stderr.trim())
	}
}

fn run_ydotool<const N: usize>(args: [&str; N]) -> Result<()> {
	let output = command_output("ydotool", args)?;
	if output.status.success() {
		Ok(())
	} else {
		let stderr = String::from_utf8_lossy(&output.stderr);
		bail!("ydotool failed: {}", stderr.trim())
	}
}

fn run_ydotool_stdin<const N: usize>(args: [&str; N], stdin: &str) -> Result<()> {
	let mut child = Command::new("ydotool")
		.args(args)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.map_err(|err| {
			if err.kind() == std::io::ErrorKind::NotFound {
				std::io::Error::new(err.kind(), "ydotool not found")
			} else {
				err
			}
		})
		.context("failed to run ydotool")?;
	if let Some(mut input) = child.stdin.take() {
		input.write_all(stdin.as_bytes())?;
	}
	let output = child.wait_with_output()?;
	if output.status.success() {
		Ok(())
	} else {
		let stderr = String::from_utf8_lossy(&output.stderr);
		bail!("ydotool failed: {}", stderr.trim())
	}
}

fn command_output<const N: usize>(command: &str, args: [&str; N]) -> Result<std::process::Output> {
	Command::new(command)
		.args(args)
		.output()
		.map_err(|err| {
			if err.kind() == std::io::ErrorKind::NotFound {
				std::io::Error::new(err.kind(), format!("{command} not found"))
			} else {
				err
			}
		})
		.with_context(|| format!("failed to run {command}"))
}

#[cfg(target_os = "linux")]
mod uinput_mouse {
	use crate::p2p::MouseButton;
	use anyhow::{Context, Result};
	use std::fs::{File, OpenOptions};
	use std::io::Write;
	use std::mem::size_of;
	use std::os::fd::AsRawFd;
	use std::os::raw::{c_char, c_int};
	use std::slice;
	use std::sync::{Mutex, OnceLock};
	use std::time::Duration;

	const BUS_USB: u16 = 0x03;
	const BTN_LEFT: u16 = 0x110;
	const BTN_MIDDLE: u16 = 0x112;
	const BTN_RIGHT: u16 = 0x111;
	const EV_KEY: u16 = 0x01;
	const EV_REL: u16 = 0x02;
	const EV_SYN: u16 = 0x00;
	const KEY_0: u16 = 11;
	const KEY_1: u16 = 2;
	const KEY_2: u16 = 3;
	const KEY_3: u16 = 4;
	const KEY_4: u16 = 5;
	const KEY_5: u16 = 6;
	const KEY_6: u16 = 7;
	const KEY_7: u16 = 8;
	const KEY_8: u16 = 9;
	const KEY_9: u16 = 10;
	const KEY_A: u16 = 30;
	const KEY_APOSTROPHE: u16 = 40;
	const KEY_B: u16 = 48;
	const KEY_BACKSLASH: u16 = 43;
	const KEY_BACKSPACE: u16 = 14;
	const KEY_C: u16 = 46;
	const KEY_COMMA: u16 = 51;
	const KEY_D: u16 = 32;
	const KEY_DOT: u16 = 52;
	const KEY_DOWN: u16 = 108;
	const KEY_E: u16 = 18;
	const KEY_ENTER: u16 = 28;
	const KEY_EQUAL: u16 = 13;
	const KEY_ESC: u16 = 1;
	const KEY_F: u16 = 33;
	const KEY_G: u16 = 34;
	const KEY_GRAVE: u16 = 41;
	const KEY_H: u16 = 35;
	const KEY_I: u16 = 23;
	const KEY_J: u16 = 36;
	const KEY_K: u16 = 37;
	const KEY_L: u16 = 38;
	const KEY_LEFT: u16 = 105;
	const KEY_LEFTBRACE: u16 = 26;
	const KEY_LEFTSHIFT: u16 = 42;
	const KEY_M: u16 = 50;
	const KEY_MINUS: u16 = 12;
	const KEY_N: u16 = 49;
	const KEY_O: u16 = 24;
	const KEY_P: u16 = 25;
	const KEY_Q: u16 = 16;
	const KEY_R: u16 = 19;
	const KEY_RIGHT: u16 = 106;
	const KEY_RIGHTBRACE: u16 = 27;
	const KEY_S: u16 = 31;
	const KEY_SEMICOLON: u16 = 39;
	const KEY_SLASH: u16 = 53;
	const KEY_SPACE: u16 = 57;
	const KEY_T: u16 = 20;
	const KEY_TAB: u16 = 15;
	const KEY_U: u16 = 22;
	const KEY_UP: u16 = 103;
	const KEY_V: u16 = 47;
	const KEY_W: u16 = 17;
	const KEY_X: u16 = 45;
	const KEY_Y: u16 = 21;
	const KEY_Z: u16 = 44;
	const REL_WHEEL: u16 = 0x08;
	const REL_X: u16 = 0x00;
	const REL_Y: u16 = 0x01;
	const SYN_REPORT: u16 = 0x00;
	const UI_DEV_CREATE: libc::c_ulong = ioctl_none(b'U', 1);
	const UI_DEV_DESTROY: libc::c_ulong = ioctl_none(b'U', 2);
	const UI_SET_EVBIT: libc::c_ulong = ioctl_write_int(b'U', 100);
	const UI_SET_KEYBIT: libc::c_ulong = ioctl_write_int(b'U', 101);
	const UI_SET_RELBIT: libc::c_ulong = ioctl_write_int(b'U', 102);
	const UINPUT_MAX_NAME_SIZE: usize = 80;

	static UINPUT_MOUSE: OnceLock<Mutex<Option<UInputMouse>>> = OnceLock::new();

	const KEYBOARD_KEYS: &[u16] = &[
		KEY_0,
		KEY_1,
		KEY_2,
		KEY_3,
		KEY_4,
		KEY_5,
		KEY_6,
		KEY_7,
		KEY_8,
		KEY_9,
		KEY_A,
		KEY_APOSTROPHE,
		KEY_B,
		KEY_BACKSLASH,
		KEY_BACKSPACE,
		KEY_C,
		KEY_COMMA,
		KEY_D,
		KEY_DOT,
		KEY_DOWN,
		KEY_E,
		KEY_ENTER,
		KEY_EQUAL,
		KEY_ESC,
		KEY_F,
		KEY_G,
		KEY_GRAVE,
		KEY_H,
		KEY_I,
		KEY_J,
		KEY_K,
		KEY_L,
		KEY_LEFT,
		KEY_LEFTBRACE,
		KEY_LEFTSHIFT,
		KEY_M,
		KEY_MINUS,
		KEY_N,
		KEY_O,
		KEY_P,
		KEY_Q,
		KEY_R,
		KEY_RIGHT,
		KEY_RIGHTBRACE,
		KEY_S,
		KEY_SEMICOLON,
		KEY_SLASH,
		KEY_SPACE,
		KEY_T,
		KEY_TAB,
		KEY_U,
		KEY_UP,
		KEY_V,
		KEY_W,
		KEY_X,
		KEY_Y,
		KEY_Z,
	];

	#[repr(C)]
	#[derive(Default)]
	struct InputId {
		bustype: u16,
		vendor: u16,
		product: u16,
		version: u16,
	}

	#[repr(C)]
	struct InputEvent {
		time: libc::timeval,
		type_: u16,
		code: u16,
		value: i32,
	}

	#[repr(C)]
	struct UInputUserDev {
		name: [c_char; UINPUT_MAX_NAME_SIZE],
		id: InputId,
		ff_effects_max: u32,
		absmax: [i32; 64],
		absmin: [i32; 64],
		absfuzz: [i32; 64],
		absflat: [i32; 64],
	}

	impl Default for UInputUserDev {
		fn default() -> Self {
			Self {
				name: [0; UINPUT_MAX_NAME_SIZE],
				id: InputId::default(),
				ff_effects_max: 0,
				absmax: [0; 64],
				absmin: [0; 64],
				absfuzz: [0; 64],
				absflat: [0; 64],
			}
		}
	}

	struct UInputMouse {
		file: File,
	}

	pub(super) fn move_relative(dx: i32, dy: i32) -> Result<()> {
		if dx == 0 && dy == 0 {
			return Ok(());
		}
		with_mouse(|mouse| {
			if dx != 0 {
				mouse.emit(EV_REL, REL_X, dx)?;
			}
			if dy != 0 {
				mouse.emit(EV_REL, REL_Y, dy)?;
			}
			mouse.sync()
		})
	}

	pub(super) fn scroll(amount: i32) -> Result<()> {
		let amount = amount.clamp(-20, 20);
		if amount == 0 {
			return Ok(());
		}
		with_mouse(|mouse| {
			mouse.emit(EV_REL, REL_WHEEL, -amount)?;
			mouse.sync()
		})
	}

	pub(super) fn click(button: MouseButton) -> Result<()> {
		let code = mouse_button_code(button);
		with_mouse(|mouse| {
			mouse.emit(EV_KEY, code, 1)?;
			mouse.sync()?;
			mouse.emit(EV_KEY, code, 0)?;
			mouse.sync()
		})
	}

	pub(super) fn press(button: MouseButton) -> Result<()> {
		let code = mouse_button_code(button);
		with_mouse(|mouse| {
			mouse.emit(EV_KEY, code, 1)?;
			mouse.sync()
		})
	}

	pub(super) fn release(button: MouseButton) -> Result<()> {
		let code = mouse_button_code(button);
		with_mouse(|mouse| {
			mouse.emit(EV_KEY, code, 0)?;
			mouse.sync()
		})
	}

	pub(super) fn type_text(text: &str) -> Result<()> {
		with_mouse(|mouse| {
			for character in text.chars() {
				let Some(key) = char_key(character) else {
					anyhow::bail!("unsupported keyboard character {character:?}");
				};
				mouse.tap_key(key.code, key.shift)?;
			}
			Ok(())
		})
	}

	pub(super) fn press_key(key: &str) -> Result<()> {
		let Some(code) = named_key_code(key) else {
			anyhow::bail!("unsupported keyboard key {key:?}");
		};
		with_mouse(|mouse| mouse.tap_key(code, false))
	}

	fn mouse_button_code(button: MouseButton) -> u16 {
		match button {
			MouseButton::Left => BTN_LEFT,
			MouseButton::Middle => BTN_MIDDLE,
			MouseButton::Right => BTN_RIGHT,
		}
	}

	struct KeyStroke {
		code: u16,
		shift: bool,
	}

	fn key(code: u16) -> Option<KeyStroke> {
		Some(KeyStroke { code, shift: false })
	}

	fn shifted_key(code: u16) -> Option<KeyStroke> {
		Some(KeyStroke { code, shift: true })
	}

	fn char_key(character: char) -> Option<KeyStroke> {
		match character {
			'a' => key(KEY_A),
			'b' => key(KEY_B),
			'c' => key(KEY_C),
			'd' => key(KEY_D),
			'e' => key(KEY_E),
			'f' => key(KEY_F),
			'g' => key(KEY_G),
			'h' => key(KEY_H),
			'i' => key(KEY_I),
			'j' => key(KEY_J),
			'k' => key(KEY_K),
			'l' => key(KEY_L),
			'm' => key(KEY_M),
			'n' => key(KEY_N),
			'o' => key(KEY_O),
			'p' => key(KEY_P),
			'q' => key(KEY_Q),
			'r' => key(KEY_R),
			's' => key(KEY_S),
			't' => key(KEY_T),
			'u' => key(KEY_U),
			'v' => key(KEY_V),
			'w' => key(KEY_W),
			'x' => key(KEY_X),
			'y' => key(KEY_Y),
			'z' => key(KEY_Z),
			'A' => shifted_key(KEY_A),
			'B' => shifted_key(KEY_B),
			'C' => shifted_key(KEY_C),
			'D' => shifted_key(KEY_D),
			'E' => shifted_key(KEY_E),
			'F' => shifted_key(KEY_F),
			'G' => shifted_key(KEY_G),
			'H' => shifted_key(KEY_H),
			'I' => shifted_key(KEY_I),
			'J' => shifted_key(KEY_J),
			'K' => shifted_key(KEY_K),
			'L' => shifted_key(KEY_L),
			'M' => shifted_key(KEY_M),
			'N' => shifted_key(KEY_N),
			'O' => shifted_key(KEY_O),
			'P' => shifted_key(KEY_P),
			'Q' => shifted_key(KEY_Q),
			'R' => shifted_key(KEY_R),
			'S' => shifted_key(KEY_S),
			'T' => shifted_key(KEY_T),
			'U' => shifted_key(KEY_U),
			'V' => shifted_key(KEY_V),
			'W' => shifted_key(KEY_W),
			'X' => shifted_key(KEY_X),
			'Y' => shifted_key(KEY_Y),
			'Z' => shifted_key(KEY_Z),
			'1' => key(KEY_1),
			'2' => key(KEY_2),
			'3' => key(KEY_3),
			'4' => key(KEY_4),
			'5' => key(KEY_5),
			'6' => key(KEY_6),
			'7' => key(KEY_7),
			'8' => key(KEY_8),
			'9' => key(KEY_9),
			'0' => key(KEY_0),
			'!' => shifted_key(KEY_1),
			'@' => shifted_key(KEY_2),
			'#' => shifted_key(KEY_3),
			'$' => shifted_key(KEY_4),
			'%' => shifted_key(KEY_5),
			'^' => shifted_key(KEY_6),
			'&' => shifted_key(KEY_7),
			'*' => shifted_key(KEY_8),
			'(' => shifted_key(KEY_9),
			')' => shifted_key(KEY_0),
			' ' => key(KEY_SPACE),
			'\n' | '\r' => key(KEY_ENTER),
			'\t' => key(KEY_TAB),
			'-' => key(KEY_MINUS),
			'_' => shifted_key(KEY_MINUS),
			'=' => key(KEY_EQUAL),
			'+' => shifted_key(KEY_EQUAL),
			'[' => key(KEY_LEFTBRACE),
			'{' => shifted_key(KEY_LEFTBRACE),
			']' => key(KEY_RIGHTBRACE),
			'}' => shifted_key(KEY_RIGHTBRACE),
			'\\' => key(KEY_BACKSLASH),
			'|' => shifted_key(KEY_BACKSLASH),
			';' => key(KEY_SEMICOLON),
			':' => shifted_key(KEY_SEMICOLON),
			'\'' => key(KEY_APOSTROPHE),
			'"' => shifted_key(KEY_APOSTROPHE),
			'`' => key(KEY_GRAVE),
			'~' => shifted_key(KEY_GRAVE),
			',' => key(KEY_COMMA),
			'<' => shifted_key(KEY_COMMA),
			'.' => key(KEY_DOT),
			'>' => shifted_key(KEY_DOT),
			'/' => key(KEY_SLASH),
			'?' => shifted_key(KEY_SLASH),
			_ => None,
		}
	}

	fn named_key_code(key: &str) -> Option<u16> {
		match key {
			"Return" => Some(KEY_ENTER),
			"Tab" => Some(KEY_TAB),
			"BackSpace" => Some(KEY_BACKSPACE),
			"Escape" => Some(KEY_ESC),
			"Up" => Some(KEY_UP),
			"Down" => Some(KEY_DOWN),
			"Left" => Some(KEY_LEFT),
			"Right" => Some(KEY_RIGHT),
			_ => None,
		}
	}

	fn with_mouse(run: impl FnOnce(&mut UInputMouse) -> Result<()>) -> Result<()> {
		let mut mouse = UINPUT_MOUSE
			.get_or_init(|| Mutex::new(None))
			.lock()
			.unwrap_or_else(|err| err.into_inner());
		if mouse.is_none() {
			*mouse = Some(UInputMouse::open()?);
		}
		run(mouse.as_mut().expect("uinput mouse initialized"))
	}

	impl UInputMouse {
		fn open() -> Result<Self> {
			let mut file = OpenOptions::new()
				.read(true)
				.write(true)
				.open("/dev/uinput")
				.context("failed to open /dev/uinput")?;
			let fd = file.as_raw_fd();
			ioctl_arg(fd, UI_SET_EVBIT, EV_KEY, "UI_SET_EVBIT EV_KEY")?;
			ioctl_arg(fd, UI_SET_EVBIT, EV_REL, "UI_SET_EVBIT EV_REL")?;
			ioctl_arg(fd, UI_SET_KEYBIT, BTN_LEFT, "UI_SET_KEYBIT BTN_LEFT")?;
			ioctl_arg(fd, UI_SET_KEYBIT, BTN_MIDDLE, "UI_SET_KEYBIT BTN_MIDDLE")?;
			ioctl_arg(fd, UI_SET_KEYBIT, BTN_RIGHT, "UI_SET_KEYBIT BTN_RIGHT")?;
			for key in KEYBOARD_KEYS {
				ioctl_arg(fd, UI_SET_KEYBIT, *key, "UI_SET_KEYBIT keyboard")?;
			}
			ioctl_arg(fd, UI_SET_RELBIT, REL_X, "UI_SET_RELBIT REL_X")?;
			ioctl_arg(fd, UI_SET_RELBIT, REL_Y, "UI_SET_RELBIT REL_Y")?;
			ioctl_arg(fd, UI_SET_RELBIT, REL_WHEEL, "UI_SET_RELBIT REL_WHEEL")?;

			let user_dev = user_dev();
			file.write_all(as_bytes(&user_dev))
				.context("failed to configure uinput mouse")?;
			ioctl_none_arg(fd, UI_DEV_CREATE, "UI_DEV_CREATE")?;
			std::thread::sleep(Duration::from_millis(50));
			Ok(Self { file })
		}

		fn emit(&mut self, type_: u16, code: u16, value: i32) -> Result<()> {
			let event = InputEvent {
				time: libc::timeval {
					tv_sec: 0,
					tv_usec: 0,
				},
				type_,
				code,
				value,
			};
			self.file
				.write_all(as_bytes(&event))
				.context("failed to write uinput event")
		}

		fn sync(&mut self) -> Result<()> {
			self.emit(EV_SYN, SYN_REPORT, 0)
		}

		fn tap_key(&mut self, code: u16, shift: bool) -> Result<()> {
			if shift {
				self.emit(EV_KEY, KEY_LEFTSHIFT, 1)?;
			}
			self.emit(EV_KEY, code, 1)?;
			self.sync()?;
			self.emit(EV_KEY, code, 0)?;
			if shift {
				self.emit(EV_KEY, KEY_LEFTSHIFT, 0)?;
			}
			self.sync()
		}
	}

	impl Drop for UInputMouse {
		fn drop(&mut self) {
			let _ = ioctl_none_arg(self.file.as_raw_fd(), UI_DEV_DESTROY, "UI_DEV_DESTROY");
		}
	}

	fn user_dev() -> UInputUserDev {
		let mut user_dev = UInputUserDev::default();
		for (target, source) in user_dev
			.name
			.iter_mut()
			.zip(b"puppynet virtual mouse\0".iter().copied())
		{
			*target = source as c_char;
		}
		user_dev.id = InputId {
			bustype: BUS_USB,
			vendor: 0x7075,
			product: 0x7079,
			version: 1,
		};
		user_dev
	}

	fn ioctl_arg(fd: c_int, request: libc::c_ulong, arg: u16, name: &str) -> Result<()> {
		let result = unsafe { libc::ioctl(fd, request, c_int::from(arg)) };
		if result < 0 {
			Err(std::io::Error::last_os_error()).with_context(|| format!("{name} failed"))
		} else {
			Ok(())
		}
	}

	fn ioctl_none_arg(fd: c_int, request: libc::c_ulong, name: &str) -> Result<()> {
		let result = unsafe { libc::ioctl(fd, request) };
		if result < 0 {
			Err(std::io::Error::last_os_error()).with_context(|| format!("{name} failed"))
		} else {
			Ok(())
		}
	}

	fn as_bytes<T>(value: &T) -> &[u8] {
		unsafe { slice::from_raw_parts(value as *const T as *const u8, size_of::<T>()) }
	}

	const fn ioctl_none(type_: u8, nr: u8) -> libc::c_ulong {
		ioctl(0, type_, nr, 0)
	}

	const fn ioctl_write_int(type_: u8, nr: u8) -> libc::c_ulong {
		ioctl(1, type_, nr, size_of::<c_int>())
	}

	const fn ioctl(dir: u8, type_: u8, nr: u8, size: usize) -> libc::c_ulong {
		((dir as libc::c_ulong) << 30)
			| ((size as libc::c_ulong) << 16)
			| ((type_ as libc::c_ulong) << 8)
			| nr as libc::c_ulong
	}
}

#[cfg(not(target_os = "linux"))]
mod uinput_mouse {
	use crate::p2p::MouseButton;
	use anyhow::{Result, bail};

	pub(super) fn move_relative(_dx: i32, _dy: i32) -> Result<()> {
		bail!("uinput mouse is only supported on Linux")
	}

	pub(super) fn scroll(_amount: i32) -> Result<()> {
		bail!("uinput mouse is only supported on Linux")
	}

	pub(super) fn click(_button: MouseButton) -> Result<()> {
		bail!("uinput mouse is only supported on Linux")
	}

	pub(super) fn press(_button: MouseButton) -> Result<()> {
		bail!("uinput mouse is only supported on Linux")
	}

	pub(super) fn release(_button: MouseButton) -> Result<()> {
		bail!("uinput mouse is only supported on Linux")
	}

	pub(super) fn type_text(_text: &str) -> Result<()> {
		bail!("uinput keyboard is only supported on Linux")
	}

	pub(super) fn press_key(_key: &str) -> Result<()> {
		bail!("uinput keyboard is only supported on Linux")
	}
}
