//! VT/ANSI escape sequence parser — Layer 2.
//!
//! Implements Paul Williams' state machine exactly as specified at:
//!   <https://vt100.net/emu/dec_ansi_parser>
//!
//! The parser is byte-driven: call [`Parser::advance`] with each byte.
//! It invokes the appropriate method on a [`Perform`] trait object to
//! deliver decoded terminal actions to the grid.
//!
//! ## States
//!
//! ```text
//! Ground ──────────────────────────────────────────────┐
//!   │ ESC                                              │
//!   ▼                                                  │
//! Escape ──────────────────────────────────────────────┤
//!   │ [            │ ]            │ P            │ other│
//!   ▼              ▼              ▼              ▼      │
//! CsiEntry  OscString    DcsEntry  EscIntermediate      │
//!   │                                                   │
//!   ▼                                                   │
//! CsiParam / CsiIntermediate / CsiIgnore                │
//! ```

/// Every terminal action emitted by the parser to the grid handler.
#[derive(Debug, Clone)]
pub enum Action {
    /// A printable Unicode character (including decoded UTF-8 multibyte).
    Print(char),
    /// A C0 or C1 control byte: `BS`=0x08, `LF`=0x0a, `CR`=0x0d, etc.
    Execute(u8),
    /// CSI sequence final dispatch.
    CsiDispatch {
        params: Vec<Vec<u16>>, // sub-params separated by ':'
        intermediates: Vec<u8>,
        ignore: bool,
        final_byte: u8,
    },
    /// OSC (Operating System Command) string complete.
    OscDispatch { params: Vec<Vec<u8>> },
    /// DCS hook (start of DCS passthrough).
    Hook {
        params: Vec<Vec<u16>>,
        intermediates: Vec<u8>,
        ignore: bool,
        final_byte: u8,
    },
    /// DCS data byte.
    Put(u8),
    /// DCS unhook (end of DCS passthrough).
    Unhook,
    /// ESC + intermediate + final (two-byte escape sequence, e.g. ESC = / ESC >).
    EscDispatch {
        intermediates: Vec<u8>,
        ignore: bool,
        final_byte: u8,
    },
}

// ---------------------------------------------------------------------------
// Perform trait — implemented by the grid to receive actions
// ---------------------------------------------------------------------------

pub trait Perform {
    fn perform(&mut self, action: Action);

    /// Batch-process a run of bytes starting from Ground state. Handles
    /// printable ASCII (0x20..=0x7e) and common control characters (LF, CR,
    /// BS, HT) inline. Returns the number of bytes consumed. Stops at the
    /// first byte that requires per-byte state machine processing (ESC,
    /// non-ASCII, etc). The default implementation calls `perform` per byte;
    /// performers with a fast batch path (like `Grid`) should override this.
    fn print_ascii_run(&mut self, bytes: &[u8]) -> usize {
        let mut consumed = 0;
        for &b in bytes {
            match b {
                0x0a => {
                    self.perform(Action::Execute(b));
                    consumed += 1;
                }
                0x0d => {
                    self.perform(Action::Execute(b));
                    consumed += 1;
                }
                0x08 => {
                    self.perform(Action::Execute(b));
                    consumed += 1;
                }
                0x09 => {
                    self.perform(Action::Execute(b));
                    consumed += 1;
                }
                0x20..=0x7e => {
                    self.perform(Action::Print(b as char));
                    consumed += 1;
                }
                _ => break,
            }
        }
        consumed
    }
}

// ---------------------------------------------------------------------------
// Parser state machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsIgnore,
    DcsPassthrough,
    SosPmApcString,
    Utf8,
}

/// Maximum number of CSI parameters.
const MAX_PARAMS: usize = 32;
/// Maximum number of intermediate bytes.
const MAX_INTERMEDIATES: usize = 2;
/// Maximum OSC string length. xterm-class allows ~100KB; the old 1024 cap
/// silently truncated long titles and OSC 52 clipboard payloads (T3-18).
const MAX_OSC_LEN: usize = 100_000;

pub struct Parser {
    state: State,

    // CSI / ESC param accumulator
    params: [[u16; 16]; MAX_PARAMS], // sub-params per param
    param_len: [usize; MAX_PARAMS],  // how many sub-params in each
    num_params: usize,
    current_param: u16, // digits being accumulated

    intermediates: [u8; MAX_INTERMEDIATES],
    num_intermediates: usize,
    ignore: bool,

    // OSC accumulator
    osc_buf: Vec<u8>,
    osc_params: Vec<usize>, // byte offsets of ';' separators

    // UTF-8 multibyte accumulator
    utf8_buf: [u8; 4],
    utf8_len: usize,
    utf8_remaining: usize,
}

impl Default for Parser {
    fn default() -> Self {
        Self {
            state: State::Ground,
            params: [[0; 16]; MAX_PARAMS],
            param_len: [0; MAX_PARAMS],
            num_params: 0,
            current_param: 0,
            intermediates: [0; MAX_INTERMEDIATES],
            num_intermediates: 0,
            ignore: false,
            osc_buf: Vec::with_capacity(256),
            osc_params: Vec::with_capacity(16),
            utf8_buf: [0; 4],
            utf8_len: 0,
            utf8_remaining: 0,
        }
    }
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one byte into the state machine, calling `performer` with any
    /// actions that result.
    pub fn advance(&mut self, performer: &mut impl Perform, byte: u8) {
        // UTF-8 multibyte in progress — collect continuation bytes
        if self.state == State::Utf8 {
            self.advance_utf8(performer, byte);
            return;
        }

        // OSC strings routinely contain raw UTF-8 (window titles, OSC 8 URIs,
        // OSC 52 payloads), so 0x80–0x9F bytes must be treated as string data,
        // not C1 controls — the anywhere rules below would otherwise terminate
        // the string mid-UTF-8 (e.g. a 0x9C continuation byte would read as
        // 8-bit ST). Termination is handled inside `osc_string` instead.
        // (Matches xterm and Alacritty's `vte` behaviour.)
        if self.state == State::OscString {
            self.osc_string(performer, byte);
            return;
        }

        // Anywhere transitions (take priority over current state)
        match byte {
            0x18 | 0x1a => {
                // CAN/SUB abort any in-progress string without dispatching it.
                performer.perform(Action::Execute(byte));
                self.transition(State::Ground);
                return;
            }
            0x1b => {
                // ESC cancels the control string in progress (Williams' spec),
                // but the pending string is complete and must be dispatched on
                // state exit: the ESC is the first byte of ST (ESC \). Dropping
                // it here silently loses every ST-terminated OSC/DCS.
                self.exit_string_states(performer);
                self.transition(State::Escape);
                return;
            }
            0x80..=0x8f | 0x91..=0x97 | 0x99 | 0x9a => {
                performer.perform(Action::Execute(byte));
                self.transition(State::Ground);
                return;
            }
            0x90 => {
                self.transition(State::DcsEntry);
                return;
            }
            0x98 | 0x9e | 0x9f => {
                self.transition(State::SosPmApcString);
                return;
            }
            0x9b => {
                self.transition(State::CsiEntry);
                return;
            }
            0x9c => {
                // ST (8-bit form) terminates the string → dispatch on exit.
                self.exit_string_states(performer);
                self.transition(State::Ground);
                return;
            }
            0x9d => {
                self.transition(State::OscString);
                return;
            }
            _ => {}
        }

        match self.state {
            State::Ground => self.ground(performer, byte),
            State::Escape => self.escape(performer, byte),
            State::EscapeIntermediate => self.escape_intermediate(performer, byte),
            State::CsiEntry => self.csi_entry(performer, byte),
            State::CsiParam => self.csi_param(performer, byte),
            State::CsiIntermediate => self.csi_intermediate(performer, byte),
            State::CsiIgnore => self.csi_ignore(performer, byte),
            State::OscString => self.osc_string(performer, byte),
            State::DcsEntry => self.dcs_entry(performer, byte),
            State::DcsParam => self.dcs_param(performer, byte),
            State::DcsIntermediate => self.dcs_intermediate(performer, byte),
            State::DcsIgnore => self.dcs_ignore(byte),
            State::DcsPassthrough => self.dcs_passthrough(performer, byte),
            State::SosPmApcString => { /* absorb */ }
            State::Utf8 => unreachable!(),
        }
    }

    /// Feed a slice of bytes into the state machine. This is the preferred
    /// entry point for bulk PTY data — when the parser is in the Ground state,
    /// it delegates to the performer's `print_ascii_run` batch method, which
    /// handles printable ASCII runs and common control characters (LF, CR, BS,
    /// HT) inline, avoiding per-byte function call and `Action` enum overhead.
    pub fn advance_bytes(&mut self, performer: &mut impl Perform, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            // Fast path: in Ground state, delegate to the performer's batch
            // method. It handles printable ASCII + common control chars and
            // returns the number of bytes consumed. Falls back to per-byte
            // processing for bytes it doesn't handle (escape sequences, etc).
            if self.state == State::Ground {
                let remaining = &bytes[i..];
                let consumed = performer.print_ascii_run(remaining);
                if consumed > 0 {
                    i += consumed;
                    continue;
                }
            }

            self.advance(performer, bytes[i]);
            i += 1;
        }
    }

    // -----------------------------------------------------------------------
    // State handlers
    // -----------------------------------------------------------------------

    fn ground(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x00..=0x17 | 0x19 | 0x1c..=0x1f => performer.perform(Action::Execute(byte)),
            0x20..=0x7e => performer.perform(Action::Print(byte as char)),
            0x7f => {} // DEL — ignore in ground
            // High bytes → start of UTF-8 multibyte sequence
            0xc0..=0xdf => self.begin_utf8(byte, 1),
            0xe0..=0xef => self.begin_utf8(byte, 2),
            0xf0..=0xf7 => self.begin_utf8(byte, 3),
            _ => {}
        }
    }

    fn escape(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x00..=0x17 | 0x19 | 0x1c..=0x1f => performer.perform(Action::Execute(byte)),
            0x20..=0x2f => {
                self.collect_intermediate(byte);
                self.transition(State::EscapeIntermediate);
            }
            0x30..=0x4f | 0x51..=0x57 | 0x59 | 0x5a | 0x5c | 0x60..=0x7e => {
                self.dispatch_esc(performer, byte);
                self.transition(State::Ground);
            }
            0x50 => self.transition(State::DcsEntry),
            0x58 | 0x5e | 0x5f => self.transition(State::SosPmApcString),
            0x5b => self.transition(State::CsiEntry),
            0x5d => self.transition(State::OscString),
            0x7f => {} // DEL
            _ => {}
        }
    }

    fn escape_intermediate(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x00..=0x17 | 0x19 | 0x1c..=0x1f => performer.perform(Action::Execute(byte)),
            0x20..=0x2f => self.collect_intermediate(byte),
            0x30..=0x7e => {
                self.dispatch_esc(performer, byte);
                self.transition(State::Ground);
            }
            0x7f => {}
            _ => {}
        }
    }

    fn csi_entry(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x00..=0x17 | 0x19 | 0x1c..=0x1f => performer.perform(Action::Execute(byte)),
            0x20..=0x2f => {
                self.collect_intermediate(byte);
                self.transition(State::CsiIntermediate);
            }
            0x30..=0x39 => {
                self.param_digit(byte);
                self.transition(State::CsiParam);
            }
            0x3a => {
                self.sub_param_separator();
                self.transition(State::CsiParam);
            }
            0x3b => {
                self.param_separator();
                self.transition(State::CsiParam);
            }
            0x3c..=0x3f => {
                self.collect_intermediate(byte);
                self.transition(State::CsiParam);
            }
            0x40..=0x7e => {
                self.dispatch_csi(performer, byte);
                self.transition(State::Ground);
            }
            0x7f => {}
            _ => {}
        }
    }

    fn csi_param(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x00..=0x17 | 0x19 | 0x1c..=0x1f => performer.perform(Action::Execute(byte)),
            0x20..=0x2f => {
                self.collect_intermediate(byte);
                self.transition(State::CsiIntermediate);
            }
            0x30..=0x39 => self.param_digit(byte),
            0x3a => self.sub_param_separator(),
            0x3b => self.param_separator(),
            0x3c..=0x3f => self.transition(State::CsiIgnore),
            0x40..=0x7e => {
                self.dispatch_csi(performer, byte);
                self.transition(State::Ground);
            }
            0x7f => {}
            _ => {}
        }
    }

    fn csi_intermediate(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x00..=0x17 | 0x19 | 0x1c..=0x1f => performer.perform(Action::Execute(byte)),
            0x20..=0x2f => self.collect_intermediate(byte),
            0x30..=0x3f => self.transition(State::CsiIgnore),
            0x40..=0x7e => {
                self.dispatch_csi(performer, byte);
                self.transition(State::Ground);
            }
            0x7f => {}
            _ => {}
        }
    }

    fn csi_ignore(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x00..=0x17 | 0x19 | 0x1c..=0x1f => performer.perform(Action::Execute(byte)),
            0x40..=0x7e => self.transition(State::Ground),
            _ => {}
        }
    }

    fn osc_string(&mut self, performer: &mut impl Perform, byte: u8) {
        // Note: this state is handled before the anywhere rules (see
        // `advance`) so 0x80–0x9F reach here as plain data. Termination is
        // therefore BEL, ESC (first byte of `ESC \` ST), or CAN/SUB abort.
        // 8-bit ST (0x9C) is deliberately NOT a terminator: it is
        // indistinguishable from a UTF-8 continuation byte and OSC payloads
        // (titles, URIs, OSC 52) are UTF-8 in practice. Matches Alacritty's
        // `vte` and xterm behaviour.
        match byte {
            0x07 => {
                // BEL terminates OSC
                self.dispatch_osc(performer);
                self.transition(State::Ground);
            }
            0x1b => {
                // ESC cancels the string, but it is complete: dispatch on
                // state exit (Williams), then continue as a fresh ESC.
                self.dispatch_osc(performer);
                self.transition(State::Escape);
            }
            0x18 | 0x1a => {
                // CAN/SUB abort the string without dispatching it.
                performer.perform(Action::Execute(byte));
                self.transition(State::Ground);
            }
            0x20..=0xff => {
                if self.osc_buf.len() < MAX_OSC_LEN {
                    if byte == b';' {
                        self.osc_params.push(self.osc_buf.len());
                    }
                    self.osc_buf.push(byte);
                }
            }
            _ => {} // other C0 controls are ignored inside OSC
        }
    }

    fn dcs_entry(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x20..=0x2f => {
                self.collect_intermediate(byte);
                self.transition(State::DcsIntermediate);
            }
            0x30..=0x39 => {
                self.param_digit(byte);
                self.transition(State::DcsParam);
            }
            0x3b => {
                self.param_separator();
                self.transition(State::DcsParam);
            }
            0x3c..=0x3f => {
                self.collect_intermediate(byte);
                self.transition(State::DcsParam);
            }
            0x40..=0x7e => {
                self.dispatch_dcs_hook(performer, byte);
                self.transition(State::DcsPassthrough);
            }
            _ => {}
        }
    }

    fn dcs_param(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x30..=0x39 => self.param_digit(byte),
            0x3b => self.param_separator(),
            0x3c..=0x3f => self.transition(State::DcsIgnore),
            0x20..=0x2f => {
                self.collect_intermediate(byte);
                self.transition(State::DcsIntermediate);
            }
            0x40..=0x7e => {
                self.dispatch_dcs_hook(performer, byte);
                self.transition(State::DcsPassthrough);
            }
            _ => {}
        }
    }

    fn dcs_intermediate(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x20..=0x2f => self.collect_intermediate(byte),
            0x30..=0x3f => self.transition(State::DcsIgnore),
            0x40..=0x7e => {
                self.dispatch_dcs_hook(performer, byte);
                self.transition(State::DcsPassthrough);
            }
            _ => {}
        }
    }

    fn dcs_ignore(&mut self, byte: u8) {
        if byte == 0x9c {
            self.transition(State::Ground);
        }
    }

    fn dcs_passthrough(&mut self, performer: &mut impl Perform, byte: u8) {
        match byte {
            0x00..=0x17 | 0x19 | 0x1c..=0x1e => performer.perform(Action::Put(byte)),
            0x20..=0x7e => performer.perform(Action::Put(byte)),
            0x9c => {
                performer.perform(Action::Unhook);
                self.transition(State::Ground);
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Accumulator helpers
    // -----------------------------------------------------------------------

    /// Dispatch any in-progress control string when leaving its state via an
    /// anywhere-rule byte (ESC or 8-bit ST). Williams' state machine performs
    /// dispatch as an *exit* action of the string state: the ESC that cancels
    /// the string also completes it (it is the first byte of `ESC \` ST), so
    /// the accumulated OSC/DCS must be delivered before the transition.
    fn exit_string_states(&mut self, performer: &mut impl Perform) {
        match self.state {
            State::OscString => self.dispatch_osc(performer),
            State::DcsPassthrough => performer.perform(Action::Unhook),
            _ => {}
        }
    }

    fn param_digit(&mut self, byte: u8) {
        self.current_param = self
            .current_param
            .saturating_mul(10)
            .saturating_add((byte - b'0') as u16);
    }

    fn param_separator(&mut self) {
        self.flush_current_param();
        if self.num_params < MAX_PARAMS - 1 {
            self.num_params += 1;
        }
    }

    fn sub_param_separator(&mut self) {
        if self.num_params < MAX_PARAMS {
            let i = self.num_params;
            let j = self.param_len[i];
            if j < 16 {
                self.params[i][j] = self.current_param;
                self.param_len[i] = j + 1;
            }
            self.current_param = 0;
        }
    }

    fn flush_current_param(&mut self) {
        let i = self.num_params;
        if i < MAX_PARAMS {
            let j = self.param_len[i];
            if j < 16 {
                self.params[i][j] = self.current_param;
                self.param_len[i] = j + 1;
            }
            self.current_param = 0;
        }
    }

    fn collect_intermediate(&mut self, byte: u8) {
        if self.num_intermediates < MAX_INTERMEDIATES {
            self.intermediates[self.num_intermediates] = byte;
            self.num_intermediates += 1;
        } else {
            self.ignore = true;
        }
    }

    // -----------------------------------------------------------------------
    // Dispatch helpers
    // -----------------------------------------------------------------------

    fn dispatch_esc(&mut self, performer: &mut impl Perform, final_byte: u8) {
        performer.perform(Action::EscDispatch {
            intermediates: self.intermediates[..self.num_intermediates].to_vec(),
            ignore: self.ignore,
            final_byte,
        });
    }

    fn dispatch_csi(&mut self, performer: &mut impl Perform, final_byte: u8) {
        self.flush_current_param();

        let mut params: Vec<Vec<u16>> = Vec::with_capacity(self.num_params + 1);
        for i in 0..=self.num_params {
            if i < MAX_PARAMS {
                params.push(self.params[i][..self.param_len[i]].to_vec());
            }
        }

        performer.perform(Action::CsiDispatch {
            params,
            intermediates: self.intermediates[..self.num_intermediates].to_vec(),
            ignore: self.ignore,
            final_byte,
        });
    }

    /// Fire `Action::Hook` when entering DCS passthrough (T3-17). The DCS
    /// parameter and intermediate bytes are delivered to the performer so it
    /// can identify the request (e.g. `1 $ q` = DECRQSS).
    fn dispatch_dcs_hook(&mut self, performer: &mut impl Perform, final_byte: u8) {
        self.flush_current_param();

        let mut params: Vec<Vec<u16>> = Vec::with_capacity(self.num_params + 1);
        for i in 0..=self.num_params {
            if i < MAX_PARAMS {
                params.push(self.params[i][..self.param_len[i]].to_vec());
            }
        }

        performer.perform(Action::Hook {
            params,
            intermediates: self.intermediates[..self.num_intermediates].to_vec(),
            ignore: self.ignore,
            final_byte,
        });
    }

    fn dispatch_osc(&mut self, performer: &mut impl Perform) {
        // Split osc_buf at recorded ';' positions
        let mut params: Vec<Vec<u8>> = Vec::new();
        let mut last = 0usize;
        for &sep in &self.osc_params {
            params.push(self.osc_buf[last..sep].to_vec());
            last = sep + 1;
        }
        params.push(self.osc_buf[last..].to_vec());
        performer.perform(Action::OscDispatch { params });
    }

    // -----------------------------------------------------------------------
    // State transition — clears accumulated state on entry to certain states
    // -----------------------------------------------------------------------

    fn transition(&mut self, next: State) {
        // Entry actions
        match next {
            State::Escape | State::CsiEntry | State::DcsEntry => {
                self.num_params = 0;
                self.param_len = [0; MAX_PARAMS];
                self.params = [[0; 16]; MAX_PARAMS];
                self.current_param = 0;
                self.num_intermediates = 0;
                self.ignore = false;
            }
            State::OscString => {
                self.osc_buf.clear();
                self.osc_params.clear();
            }
            _ => {}
        }
        self.state = next;
    }

    // -----------------------------------------------------------------------
    // UTF-8 multibyte handling
    // -----------------------------------------------------------------------

    fn begin_utf8(&mut self, byte: u8, remaining: usize) {
        self.utf8_buf[0] = byte;
        self.utf8_len = 1;
        self.utf8_remaining = remaining;
        self.state = State::Utf8;
    }

    fn advance_utf8(&mut self, performer: &mut impl Perform, byte: u8) {
        if byte & 0xc0 != 0x80 {
            // Not a continuation byte — bail, emit replacement char
            performer.perform(Action::Print('\u{FFFD}'));
            self.state = State::Ground;
            self.advance(performer, byte); // reprocess
            return;
        }
        self.utf8_buf[self.utf8_len] = byte;
        self.utf8_len += 1;
        self.utf8_remaining -= 1;

        if self.utf8_remaining == 0 {
            let s = std::str::from_utf8(&self.utf8_buf[..self.utf8_len]);
            match s {
                Ok(s) => {
                    if let Some(ch) = s.chars().next() {
                        performer.perform(Action::Print(ch));
                    }
                }
                Err(_) => performer.perform(Action::Print('\u{FFFD}')),
            }
            self.state = State::Ground;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple Perform implementation that collects all actions.
    struct Collector {
        actions: Vec<Action>,
    }

    impl Collector {
        fn new() -> Self {
            Collector {
                actions: Vec::new(),
            }
        }

        fn drain(&mut self) -> Vec<Action> {
            std::mem::take(&mut self.actions)
        }

        fn last(&self) -> Option<&Action> {
            self.actions.last()
        }

        fn count(&self) -> usize {
            self.actions.len()
        }
    }

    impl Perform for Collector {
        fn perform(&mut self, action: Action) {
            self.actions.push(action);
        }
    }

    fn feed(parser: &mut Parser, performer: &mut Collector, input: &[u8]) {
        for &b in input {
            parser.advance(performer, b);
        }
    }

    // -- Basic printing --

    #[test]
    fn test_print_ascii() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"ABC");
        assert_eq!(c.actions.len(), 3);
        match &c.actions[0] {
            Action::Print(ch) => assert_eq!(*ch, 'A'),
            other => panic!("expected Print, got {:?}", other),
        }
    }

    #[test]
    fn test_print_space_and_del() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b" A");
        // space is printable (0x20)
        assert_eq!(c.actions.len(), 2);
        match &c.actions[0] {
            Action::Print(ch) => assert_eq!(*ch, ' '),
            other => panic!("expected Print(' '), got {:?}", other),
        }
    }

    // -- Control characters --

    #[test]
    fn test_execute_c0() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x08"); // BS
        assert_eq!(c.actions.len(), 1);
        match &c.actions[0] {
            Action::Execute(byte) => assert_eq!(*byte, 0x08),
            other => panic!("expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn test_bel_does_not_crash() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x07"); // BEL
                                       // BEL is a C0 control, should be emitted as Execute
        assert_eq!(c.actions.len(), 1);
    }

    // -- ESC sequences --

    #[test]
    fn test_escape_simple() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1bc"); // ESC c = RIS
        assert_eq!(c.actions.len(), 1);
        match &c.actions[0] {
            Action::EscDispatch {
                final_byte,
                intermediates,
                ..
            } => {
                assert_eq!(*final_byte, b'c');
                assert!(intermediates.is_empty());
            }
            other => panic!("expected EscDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_escape_intermediate() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b(0"); // ESC ( 0 = designate G0 charset
        match &c.actions[0] {
            Action::EscDispatch {
                final_byte,
                intermediates,
                ..
            } => {
                assert_eq!(*final_byte, b'0');
                assert_eq!(intermediates, &[b'(']);
            }
            other => panic!("expected EscDispatch, got {:?}", other),
        }
    }

    // -- CSI sequences --

    #[test]
    fn test_csi_cursor_position() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[10;20H"); // CUP row=10 col=20
        assert_eq!(c.actions.len(), 1);
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'H');
                assert_eq!(params.len(), 2);
                assert_eq!(params[0], vec![10]);
                assert_eq!(params[1], vec![20]);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_csi_cursor_up() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[5A"); // CUU 5
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'A');
                assert_eq!(params[0], vec![5]);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_csi_erase_line() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[2K"); // EL entire line
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'K');
                assert_eq!(params[0], vec![2]);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_csi_no_params() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[H"); // CUP with no params (defaults to 1;1)
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'H');
                // Should have one param with value 0 (default)
                assert_eq!(params.len(), 1);
                assert_eq!(params[0], vec![0]);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    // -- SGR (Select Graphic Rendition) --

    #[test]
    fn test_sgr_reset() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[0m");
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'm');
                assert_eq!(params[0], vec![0]);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_sgr_bold_and_color() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[1;31m"); // bold + red
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'm');
                assert_eq!(params.len(), 2);
                assert_eq!(params[0], vec![1]);
                assert_eq!(params[1], vec![31]);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_sgr_256_color() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[38;5;200m"); // fg 256-color
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'm');
                // 38;5;200 — the parser collects these as separate params
                // param 0 = 38, param 1 = 5, param 2 = 200
                assert!(params.len() >= 2);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_sgr_truecolor() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[38;2;255;128;0m"); // fg truecolor
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'm');
                assert!(params.len() >= 2);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    // -- Private modes (?h / ?l) --

    #[test]
    fn test_private_mode_set() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[?25h"); // show cursor
        match &c.actions[0] {
            Action::CsiDispatch {
                params,
                intermediates,
                final_byte,
                ..
            } => {
                assert_eq!(*final_byte, b'h');
                assert_eq!(intermediates, &[b'?']);
                assert_eq!(params[0], vec![25]);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_private_mode_reset() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[?25l"); // hide cursor
        match &c.actions[0] {
            Action::CsiDispatch {
                final_byte,
                intermediates,
                ..
            } => {
                assert_eq!(*final_byte, b'l');
                assert_eq!(intermediates, &[b'?']);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_private_mode_alt_screen() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b[?1049h"); // enter alt screen
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'h');
                assert_eq!(params[0], vec![1049]);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    // -- UTF-8 --

    #[test]
    fn test_utf8_2byte() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        // 'ñ' = U+00F1 = 0xC3 0xB1
        feed(&mut p, &mut c, b"\xc3\xb1");
        assert_eq!(c.actions.len(), 1);
        match &c.actions[0] {
            Action::Print(ch) => assert_eq!(*ch, 'ñ'),
            other => panic!("expected Print, got {:?}", other),
        }
    }

    #[test]
    fn test_utf8_3byte() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        // '€' = U+20AC = 0xE2 0x82 0xAC
        feed(&mut p, &mut c, b"\xe2\x82\xac");
        assert_eq!(c.actions.len(), 1);
        match &c.actions[0] {
            Action::Print(ch) => assert_eq!(*ch, '€'),
            other => panic!("expected Print, got {:?}", other),
        }
    }

    #[test]
    fn test_utf8_4byte() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        // '😀' = U+1F600 = 0xF0 0x9F 0x98 0x80
        feed(&mut p, &mut c, b"\xf0\x9f\x98\x80");
        assert_eq!(c.actions.len(), 1);
        match &c.actions[0] {
            Action::Print(ch) => assert_eq!(*ch, '😀'),
            other => panic!("expected Print, got {:?}", other),
        }
    }

    #[test]
    fn test_advance_bytes_handles_malformed_control_stream() {
        let mut parser = Parser::new();
        let mut collector = Collector::new();
        let mut input = Vec::with_capacity(120_000);
        input.extend_from_slice(b"\x1b]");
        input.extend(std::iter::repeat(b'x').take(110_000));
        input.extend_from_slice(b"\x07after\x1bP1$r\x1b\\done");

        parser.advance_bytes(&mut collector, &input);

        assert!(collector
            .actions
            .iter()
            .any(|action| { matches!(action, Action::Print('a')) }));
        assert!(collector
            .actions
            .iter()
            .any(|action| { matches!(action, Action::Print('d')) }));
    }

    #[test]
    fn test_advance_bytes_handles_every_byte_without_panicking() {
        let mut parser = Parser::new();
        let mut collector = Collector::new();
        let input: Vec<u8> = (0..=u8::MAX).cycle().take(16_384).collect();

        parser.advance_bytes(&mut collector, &input);
    }

    #[test]
    fn test_utf8_invalid_sequence() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        // Invalid: 0xC0 (start of 2-byte) followed by non-continuation byte
        feed(&mut p, &mut c, b"\xc0\x41");
        // Should emit replacement char + reprocess 'A'
        assert!(c.actions.len() >= 2);
        match &c.actions[0] {
            Action::Print(ch) => assert_eq!(*ch, '\u{FFFD}'),
            other => panic!("expected Print(replacement), got {:?}", other),
        }
    }

    // -- OSC sequences --

    #[test]
    fn test_osc_title() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        // OSC 0 ; title BEL
        feed(&mut p, &mut c, b"\x1b]0;My Title\x07");
        assert_eq!(c.actions.len(), 1);
        match &c.actions[0] {
            Action::OscDispatch { params } => {
                assert_eq!(params.len(), 2);
                assert_eq!(params[0], b"0");
                assert_eq!(params[1], b"My Title");
            }
            other => panic!("expected OscDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_osc_terminates_on_bel() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b]0;title\x07");
        assert_eq!(c.actions.len(), 1);
    }

    #[test]
    fn test_osc_terminates_on_st() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        // OSC terminated by ST (ESC \). Williams' spec: the ESC cancels the
        // string but dispatch happens on state exit, so the OSC must be
        // delivered with its content, followed by the ESC dispatch.
        feed(&mut p, &mut c, b"\x1b]0;title\x1b\\");
        assert_eq!(c.actions.len(), 2, "OSC dispatch + ESC dispatch");
        match &c.actions[0] {
            Action::OscDispatch { params } => {
                assert_eq!(params.len(), 2);
                assert_eq!(params[0], b"0");
                assert_eq!(params[1], b"title");
            }
            other => panic!("expected OscDispatch with content, got {:?}", other),
        }
        match &c.actions[1] {
            Action::EscDispatch { final_byte, .. } => assert_eq!(*final_byte, b'\\'),
            other => panic!("expected EscDispatch(\\), got {:?}", other),
        }
    }

    #[test]
    fn test_osc_multibyte_utf8_terminated_on_st() {
        // Mirrors Alacritty vte's osc_containing_string_terminator test:
        // multibyte UTF-8 inside an ST-terminated OSC must survive intact.
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b]2;\xe6\x9c\xaa\x1b\\");
        match &c.actions[0] {
            Action::OscDispatch { params } => {
                assert_eq!(params[0], b"2");
                assert_eq!(params[1], b"\xe6\x9c\xaa");
            }
            other => panic!("expected OscDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_osc_8bit_st_byte_treated_as_data() {
        // 0x9C is the 8-bit ST, but inside an OSC it is indistinguishable
        // from a UTF-8 continuation byte. We follow xterm/vte and treat it
        // as string data; the OSC is terminated by BEL/ESC instead.
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b]0;ti\x9ctle\x07");
        assert_eq!(c.actions.len(), 1);
        match &c.actions[0] {
            Action::OscDispatch { params } => {
                assert_eq!(params[0], b"0");
                assert_eq!(params[1], b"ti\x9ctle"); // 0x9C preserved as data
            }
            other => panic!("expected OscDispatch, got {:?}", other),
        }
    }

    #[test]
    fn test_osc_aborted_by_can_not_dispatched() {
        // CAN (0x18) aborts the string WITHOUT dispatching it.
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"\x1b]0;title\x18");
        // Only the Execute(CAN); no OscDispatch.
        assert_eq!(c.actions.len(), 1);
        match &c.actions[0] {
            Action::Execute(byte) => assert_eq!(*byte, 0x18),
            other => panic!("expected Execute(CAN), got {:?}", other),
        }
    }

    // -- DECSC / DECRC (save/restore cursor) --

    #[test]
    fn test_decsc_decrc() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        // ESC 7 = DECSC, ESC 8 = DECRC
        feed(&mut p, &mut c, b"\x1b7\x1b8");
        assert_eq!(c.actions.len(), 2);
        match &c.actions[0] {
            Action::EscDispatch { final_byte, .. } => assert_eq!(*final_byte, b'7'),
            other => panic!("expected EscDispatch(7), got {:?}", other),
        }
        match &c.actions[1] {
            Action::EscDispatch { final_byte, .. } => assert_eq!(*final_byte, b'8'),
            other => panic!("expected EscDispatch(8), got {:?}", other),
        }
    }

    // -- Mixed sequences --

    #[test]
    fn test_mixed_text_and_csi() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        feed(&mut p, &mut c, b"Hello\x1b[1mWorld");
        // H e l l o ESC [ 1 m W o r l d
        assert_eq!(c.actions.len(), 11);
        // Check first char
        match &c.actions[0] {
            Action::Print(ch) => assert_eq!(*ch, 'H'),
            other => panic!("expected Print, got {:?}", other),
        }
        // Check SGR
        match &c.actions[5] {
            Action::CsiDispatch { final_byte, .. } => assert_eq!(*final_byte, b'm'),
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }

    // -- State machine edge cases --

    #[test]
    fn test_consecutive_escapes() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        // Two ESC sequences back to back
        feed(&mut p, &mut c, b"\x1b[H\x1b[2J");
        assert_eq!(c.actions.len(), 2);
    }

    #[test]
    fn test_csi_param_overflow() {
        let mut p = Parser::new();
        let mut c = Collector::new();
        // Very large param — should not panic
        feed(&mut p, &mut c, b"\x1b[99999H");
        match &c.actions[0] {
            Action::CsiDispatch {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, b'H');
                // 99999 clamped to u16::MAX? No, it saturates at u16
                assert_eq!(params[0], vec![99999.min(u16::MAX as u32) as u16]);
            }
            other => panic!("expected CsiDispatch, got {:?}", other),
        }
    }
}
