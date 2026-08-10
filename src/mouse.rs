//! Mouse event encoding — converts winit mouse events to terminal CSI sequences.

use crate::grid::{MouseEncoding, MouseMode};

/// Mouse button identifiers for terminal encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    Other(u16),
}

/// Mouse event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventType {
    Press,
    Release,
    Motion,
}

/// Encoded mouse event ready to send to PTY.
#[derive(Debug, Clone)]
pub struct MouseEvent {
    pub button: MouseButton,
    pub event_type: MouseEventType,
    pub col: u32,
    pub row: u32,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl MouseEvent {
    /// Encode this mouse event as a CSI sequence string.
    pub fn encode(&self, encoding: MouseEncoding) -> String {
        match encoding {
            MouseEncoding::X10 => self.encode_x10(),
            MouseEncoding::SGR => self.encode_sgr(),
        }
    }

    /// Legacy X10 encoding: CSI M Cb Cx Cy
    fn encode_x10(&self) -> String {
        let mut cb: u8 = match self.button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::WheelUp => 64,
            MouseButton::WheelDown => 65,
            MouseButton::WheelLeft => 66,
            MouseButton::WheelRight => 67,
            MouseButton::Other(b) => b as u8,
        };

        // Add motion flag
        if self.event_type == MouseEventType::Motion {
            cb += 32;
        }

        // Add modifier flags
        if self.shift { cb += 4; }
        if self.ctrl { cb += 8; }
        if self.alt { cb += 16; }

        // Release encodes as button 3 in legacy mode
        let cb_final = if self.event_type == MouseEventType::Release {
            3
        } else {
            cb
        };

        // All X10 bytes are offset by 32 (space)
        let cb_byte = cb_final + 32;
        let cx = (self.col as u8 + 32).min(255);
        let cy = (self.row as u8 + 32).min(255);

        format!("\x1b[M{}{}{}", cb_byte as char, cx as char, cy as char)
    }

    /// SGR extended encoding: CSI < Cb ; Cx ; Cy M (press) or m (release)
    fn encode_sgr(&self) -> String {
        let mut cb: u32 = match self.button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::WheelUp => 64,
            MouseButton::WheelDown => 65,
            MouseButton::WheelLeft => 66,
            MouseButton::WheelRight => 67,
            MouseButton::Other(b) => b as u32,
        };

        // Add motion flag
        if self.event_type == MouseEventType::Motion {
            cb += 32;
        }

        // Add modifier flags
        if self.shift { cb += 4; }
        if self.ctrl { cb += 8; }
        if self.alt { cb += 16; }

        let final_char = if self.event_type == MouseEventType::Release {
            'm'
        } else {
            'M'
        };

        // SGR uses 1-based coordinates
        format!("\x1b[<{};{};{}{}", cb, self.col, self.row, final_char)
    }
}

/// Encode a winit mouse button press/release into a terminal mouse event.
pub fn encode_mouse_event(
    button: Option<MouseButton>,
    event_type: MouseEventType,
    col: u32,
    row: u32,
    shift: bool,
    ctrl: bool,
    alt: bool,
) -> Option<MouseEvent> {
    let button = button?;
    Some(MouseEvent {
        button,
        event_type,
        col,
        row,
        shift,
        ctrl,
        alt,
    })
}

/// Check if mouse tracking is active for the given mode.
pub fn is_mouse_tracking_active(mode: MouseMode) -> bool {
    mode != MouseMode::None
}

/// Check if motion events should be reported based on mouse mode.
pub fn should_report_motion(mode: MouseMode, button_pressed: bool) -> bool {
    match mode {
        MouseMode::None => false,
        MouseMode::Normal => false, // Normal only reports press/release
        MouseMode::ButtonEvent => button_pressed, // Motion only while button held
        MouseMode::AnyEvent => true, // All motion
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sgr_left_click() {
        let event = MouseEvent {
            button: MouseButton::Left,
            event_type: MouseEventType::Press,
            col: 10,
            row: 5,
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(event.encode(MouseEncoding::SGR), "\x1b[<0;10;5M");
    }

    #[test]
    fn test_sgr_left_release() {
        let event = MouseEvent {
            button: MouseButton::Left,
            event_type: MouseEventType::Release,
            col: 10,
            row: 5,
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(event.encode(MouseEncoding::SGR), "\x1b[<0;10;5m");
    }

    #[test]
    fn test_sgr_right_click_with_ctrl() {
        let event = MouseEvent {
            button: MouseButton::Right,
            event_type: MouseEventType::Press,
            col: 20,
            row: 15,
            shift: false,
            ctrl: true,
            alt: false,
        };
        // Right=2 + ctrl=8 = 10
        assert_eq!(event.encode(MouseEncoding::SGR), "\x1b[<10;20;15M");
    }

    #[test]
    fn test_sgr_motion() {
        let event = MouseEvent {
            button: MouseButton::Left,
            event_type: MouseEventType::Motion,
            col: 30,
            row: 25,
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(event.encode(MouseEncoding::SGR), "\x1b[<32;30;25M");
    }

    #[test]
    fn test_sgr_wheel_up() {
        let event = MouseEvent {
            button: MouseButton::WheelUp,
            event_type: MouseEventType::Press,
            col: 5,
            row: 5,
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(event.encode(MouseEncoding::SGR), "\x1b[<64;5;5M");
    }

    #[test]
    fn test_x10_left_click() {
        let event = MouseEvent {
            button: MouseButton::Left,
            event_type: MouseEventType::Press,
            col: 10,
            row: 5,
            shift: false,
            ctrl: false,
            alt: false,
        };
        let encoded = event.encode(MouseEncoding::X10);
        // Left=0+32=32=' ', col=10+32=42='*', row=5+32=37='%'
        assert_eq!(encoded, "\x1b[M *%");
    }

    #[test]
    fn test_x10_release() {
        let event = MouseEvent {
            button: MouseButton::Left,
            event_type: MouseEventType::Release,
            col: 10,
            row: 5,
            shift: false,
            ctrl: false,
            alt: false,
        };
        let encoded = event.encode(MouseEncoding::X10);
        // Release should encode button as 3
        assert!(encoded.contains('\x1b'));
        assert!(encoded.contains('M'));
    }

    #[test]
    fn test_should_report_motion() {
        assert!(!should_report_motion(MouseMode::None, false));
        assert!(!should_report_motion(MouseMode::Normal, false));
        assert!(!should_report_motion(MouseMode::ButtonEvent, false));
        assert!(should_report_motion(MouseMode::ButtonEvent, true));
        assert!(should_report_motion(MouseMode::AnyEvent, false));
        assert!(should_report_motion(MouseMode::AnyEvent, true));
    }
}