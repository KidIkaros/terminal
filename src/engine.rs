//! Per-tab terminal engine.
//!
//! The engine owns the grid, parser, and PTY for one tab and runs them on a
//! dedicated background thread, so parsing never blocks the render/input
//! loop. After every parse batch (or serviced command) it publishes an
//! immutable [`GridSnapshot`] — rows are shared `Arc` handles, so publishing
//! copies only row pointers — and wakes the event loop to redraw. The app
//! thread renders from the latest snapshot and drives the engine through a
//! command channel; one-way events (bell, title, clipboard, notifications)
//! flow back over an event channel.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError};

use crate::grid::{Grid, GridSnapshot, WinSize};
use crate::parser::Parser;
use crate::pty::{self, PtyHandle, PtyWriter};

/// Commands from the app thread to the engine thread. The channel preserves
/// order, so key input written via `WriteToPty` stays in sequence.
pub enum Command {
    /// Resize the grid and the PTY window.
    Resize(WinSize),
    /// Set the viewport scroll offset directly (page jumps).
    ScrollTo { offset: usize, fraction: f32 },
    /// Change the scroll offset by `amount` lines (positive = up), clamped
    /// to the scrollback bounds.
    ScrollBy { amount: i32 },
    /// Smooth-scroll up by a fractional number of lines (mouse wheel).
    SmoothScrollUp(f32),
    /// Smooth-scroll down by a fractional number of lines (mouse wheel).
    SmoothScrollDown(f32),
    /// Return the viewport to the live (bottom) position.
    ResetScroll,
    /// DECSET 1004 focus-in event — the grid queues the `CSI I` response
    /// itself and the engine writes it to the PTY.
    FocusIn,
    /// DECSET 1004 focus-out event.
    FocusOut,
    /// Write raw bytes to the PTY (key input, paste, clipboard replies).
    WriteToPty(Vec<u8>),
    /// Extract all visible lines as strings (search / selection text). The
    /// reply is sent on the included channel. Blocking, so it must not be
    /// used on the hot path.
    GetLines(Sender<Vec<String>>),
    /// Resolve the hyperlink at a grid cell (middle/left click). Reply with
    /// `None` when the cell is not part of a hyperlink.
    GetHyperlinkAt {
        col: usize,
        row: usize,
        reply: Sender<Option<String>>,
    },
    /// Jump to the previous (`dir < 0`) or next (`dir > 0`) OSC 133 prompt
    /// marker relative to the shell cursor, scrolling the viewport or moving
    /// the cursor as needed.
    JumpPrompt { dir: i8 },
    /// Stop the engine thread and drop the tab.
    Quit,
}

/// One-way events from the engine to the app thread.
pub enum Event {
    /// OSC 52 set request — the app gates it by security policy and applies
    /// it to the system clipboard.
    ClipboardSet(String),
    /// OSC 52 query — the app replies through `Command::WriteToPty`.
    ClipboardQueryRequested,
    /// Window title changed (OSC 0/2).
    TitleChanged(String),
    /// BEL / OSC 9;7 — the app flashes the window or beeps per config.
    Bell,
    /// OSC 9 notification message.
    Notification(String),
    /// DECCOLM/DECSCPP asked for a different column count — the app mirrors
    /// it in the real window so the resize round-trip re-syncs the grid.
    WindowResizeRequest(WinSize),
    /// The PTY channel closed (shell exited).
    ChannelClosed,
}

/// Handle to a running engine thread, owned by the tab manager.
pub struct EngineHandle {
    cmd_tx: Sender<Command>,
    snapshot: Arc<Mutex<Arc<GridSnapshot>>>,
    events: Receiver<Event>,
    /// Gate for the PTY reader thread (background tabs pause reading so the
    /// kernel PTY buffer provides backpressure instead of the channel).
    pub reading: Arc<AtomicBool>,
    /// Monotonic counter bumped on every snapshot publish.
    pub generation: Arc<AtomicU64>,
}

impl EngineHandle {
    /// Spawn an engine with a real PTY (shell or `-e` command).
    pub fn spawn(
        title: &str,
        size: WinSize,
        scrollback: usize,
        cell_size: (u32, u32),
        argv: &[String],
        wake: pty::WakeCallback,
        drain_budget: usize,
    ) -> Result<Self, pty::PtyError> {
        let (writer, handle, rx, reading) = pty::spawn_pty(size, argv, Box::new(|| {}))?;
        let mut grid = Grid::new(size, scrollback);
        grid.set_cell_size(cell_size.0, cell_size.1);
        Ok(Self::spawn_with(
            grid,
            Parser::new(),
            Some(writer),
            Some(handle),
            rx,
            reading,
            title,
            wake,
            drain_budget,
        ))
    }

    /// Spawn an engine without a PTY (tests). The snapshot reflects the
    /// initial grid; commands still work.
    pub fn idle(size: WinSize, scrollback: usize) -> Self {
        let grid = Grid::new(size, scrollback);
        let (_, rx) = crossbeam_channel::unbounded();
        Self::spawn_with(
            grid,
            Parser::new(),
            None,
            None,
            rx,
            Arc::new(AtomicBool::new(true)),
            "idle",
            Box::new(|| {}),
            256 * 1024,
        )
    }

    fn spawn_with(
        mut grid: Grid,
        mut parser: Parser,
        pty_writer: Option<PtyWriter>,
        pty_handle: Option<PtyHandle>,
        pty_rx: Receiver<Vec<u8>>,
        reading: Arc<AtomicBool>,
        _title: &str,
        wake: pty::WakeCallback,
        drain_budget: usize,
    ) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let snapshot: Arc<Mutex<Arc<GridSnapshot>>> =
            Arc::new(Mutex::new(Arc::new(grid.snapshot())));
        let generation = Arc::new(AtomicU64::new(0));
        let gen_clone = Arc::clone(&generation);
        let snap_clone = Arc::clone(&snapshot);
        let reading_clone = Arc::clone(&reading);
        std::thread::Builder::new()
            .name("terminal-engine".to_string())
            .spawn(move || {
                engine_loop(
                    &mut grid,
                    &mut parser,
                    pty_writer,
                    pty_rx,
                    cmd_rx,
                    events_tx,
                    snap_clone,
                    gen_clone,
                    wake,
                    drain_budget,
                );
                // Dropping the handle here (on the engine thread, at exit)
                // sends SIGHUP + reaps the child; never on the main thread,
                // where the blocking waitpid would stall the event loop.
                drop(pty_handle);
                let _ = reading_clone; // keep the gate alive with the handle
            })
            .expect("spawn engine thread");
        Self {
            cmd_tx,
            snapshot,
            events: events_rx,
            reading,
            generation,
        }
    }

    /// Latest snapshot (clone the Arc — O(1)).
    pub fn snapshot(&self) -> Arc<GridSnapshot> {
        Arc::clone(&self.snapshot.lock().unwrap())
    }

    /// Send a command (non-blocking; the engine services it promptly).
    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Stop the engine thread (dropping the tab).
    pub fn shutdown(&self) {
        self.send(Command::Quit);
    }

    /// Next pending event, if any.
    pub fn try_event(&self) -> Option<Event> {
        match self.events.try_recv() {
            Ok(ev) => Some(ev),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    /// Blocking round-trip: extract all visible lines (search/selection).
    /// Bounded wait so a stalled shell write cannot hang the app thread.
    pub fn get_lines_blocking(&self) -> Vec<String> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.send(Command::GetLines(tx));
        rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default()
    }

    /// Blocking round-trip: hyperlink at a cell.
    pub fn get_hyperlink_blocking(&self, col: usize, row: usize) -> Option<String> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.send(Command::GetHyperlinkAt {
            col,
            row,
            reply: tx,
        });
        rx.recv_timeout(Duration::from_secs(2)).ok().flatten()
    }
}

fn engine_loop(
    grid: &mut Grid,
    parser: &mut Parser,
    pty_writer: Option<PtyWriter>,
    pty_rx: Receiver<Vec<u8>>,
    cmd_rx: Receiver<Command>,
    events: Sender<Event>,
    snapshot: Arc<Mutex<Arc<GridSnapshot>>>,
    generation: Arc<AtomicU64>,
    wake: pty::WakeCallback,
    drain_budget: usize,
) {
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut channel_closed = false;
    let mut last_title = String::new();

    loop {
        // 1. Service commands (highest priority, between batches).
        let mut command_dirty = false;
        let mut quit = false;
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Command::Resize(size) => {
                    grid.resize(size);
                    if let Some(w) = &pty_writer {
                        w.resize(size);
                    }
                    command_dirty = true;
                }
                Command::ScrollTo { offset, fraction } => {
                    grid.set_scroll_offset(offset);
                    grid.set_scroll_fraction(fraction);
                    command_dirty = true;
                }
                Command::ScrollBy { amount } => {
                    let cur = grid.scrollback_offset;
                    let target = if amount >= 0 {
                        (cur + amount as usize).min(grid.scrollback.len())
                    } else {
                        cur.saturating_sub((-amount) as usize)
                    };
                    grid.set_scroll_offset(target);
                    grid.set_scroll_fraction(0.0);
                    command_dirty = true;
                }
                Command::SmoothScrollUp(lines) => {
                    grid.smooth_scroll_up(lines);
                    command_dirty = true;
                }
                Command::SmoothScrollDown(lines) => {
                    grid.smooth_scroll_down(lines);
                    command_dirty = true;
                }
                Command::ResetScroll => {
                    grid.reset_scroll();
                    command_dirty = true;
                }
                Command::FocusIn => {
                    grid.focus_in();
                    for response in grid.take_responses() {
                        if let Some(w) = &pty_writer {
                            w.write(&response);
                        }
                    }
                }
                Command::FocusOut => {
                    grid.focus_out();
                    for response in grid.take_responses() {
                        if let Some(w) = &pty_writer {
                            w.write(&response);
                        }
                    }
                }
                Command::WriteToPty(data) => {
                    if let Some(w) = &pty_writer {
                        w.write(&data);
                    }
                }
                Command::GetLines(reply) => {
                    let _ = reply.send(grid.all_lines());
                }
                Command::GetHyperlinkAt { col, row, reply } => {
                    let _ = reply.send(grid.get_hyperlink_at(col, row).map(|s| s.to_string()));
                }
                Command::JumpPrompt { dir } => {
                    let sb_len = grid.scrollback.len();
                    let from = sb_len + grid.cursor.row;
                    let target = if dir > 0 {
                        grid.next_prompt(from)
                    } else {
                        grid.prev_prompt(from)
                    };
                    match target {
                        Some(idx) if idx < sb_len => {
                            grid.set_scroll_offset(sb_len - idx);
                            grid.set_scroll_fraction(0.0);
                        }
                        Some(idx) => {
                            grid.reset_scroll();
                            grid.cursor.row = idx - sb_len;
                        }
                        None if dir > 0 => {
                            // No further prompt — return to the live view.
                            grid.reset_scroll();
                        }
                        None => {}
                    }
                    command_dirty = true;
                }
                Command::Quit => {
                    quit = true;
                    break;
                }
            }
        }
        if quit {
            return;
        }

        // 2. Drain the PTY channel up to the budget, then parse.
        let mut budget = 0usize;
        let mut drained = false;
        loop {
            match pty_rx.try_recv() {
                Ok(chunk) => {
                    budget += chunk.len();
                    pending.push(chunk);
                    drained = true;
                    if budget >= drain_budget {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    channel_closed = true;
                    break;
                }
            }
        }

        // 3. Parse + apply, then emit side effects as events.
        let trace = std::env::var_os("TERMINAL_RENDER_TRACE").is_some();
        let parse_start = trace.then(std::time::Instant::now);
        let mut parsed_bytes = 0usize;
        if drained {
            grid.bulk_output = true;
            for chunk in pending.drain(..) {
                parsed_bytes += chunk.len();
                parser.advance_bytes(grid, &chunk);
            }
            grid.bulk_output = false;

            // Device-query responses (DA/DSR/CPR/DECRQM) go straight back.
            for response in grid.take_responses() {
                if let Some(w) = &pty_writer {
                    w.write(&response);
                }
            }
            // OSC 52 clipboard (the app gates by security policy).
            if let Some(text) = grid.clipboard_set.take() {
                let _ = events.send(Event::ClipboardSet(text));
            }
            if grid.clipboard_query_requested {
                grid.clipboard_query_requested = false;
                let _ = events.send(Event::ClipboardQueryRequested);
            }
            // DECCOLM/DECSCPP column-count request — mirror in the window.
            if let Some(size) = grid.window_resize_request.take() {
                let _ = events.send(Event::WindowResizeRequest(size));
            }
            if grid.take_bell() {
                let _ = events.send(Event::Bell);
            }
            if let Some(msg) = grid.take_notification() {
                let _ = events.send(Event::Notification(msg));
            }
            // OSC 0/2 title changes.
            let title = grid.palette.title.clone();
            if title != last_title && !title.is_empty() {
                last_title = title.clone();
                let _ = events.send(Event::TitleChanged(title));
            }
        }
        if channel_closed {
            let _ = events.send(Event::ChannelClosed);
        }

        if let Some(start) = parse_start {
            if parsed_bytes > 0 {
                log::info!(
                    "perf engine parse bytes={} elapsed_us={} snapshots={}",
                    parsed_bytes,
                    start.elapsed().as_micros(),
                    generation.load(Ordering::Acquire)
                );
            }
        }

        // 4. Publish a fresh snapshot when anything changed.
        if drained || command_dirty || channel_closed {
            *snapshot.lock().unwrap() = Arc::new(grid.snapshot());
            generation.fetch_add(1, Ordering::Release);
            wake();
        }

        if channel_closed && pending.is_empty() && cmd_rx.is_empty() {
            return;
        }

        // 5. Idle: wait briefly for PTY data (the reader thread pushes
        // chunks) or commands. A short timeout keeps command latency ~1ms.
        if !drained && !command_dirty {
            match pty_rx.recv_timeout(Duration::from_millis(2)) {
                Ok(_) => {}
                Err(
                    crossbeam_channel::RecvTimeoutError::Timeout
                    | crossbeam_channel::RecvTimeoutError::Disconnected,
                ) => {}
            }
        }
    }
}
