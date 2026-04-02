use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use codex_utils_pty::ProcessHandle;
use codex_utils_pty::SpawnedProcess;
use codex_utils_pty::TerminalSize;
use color_eyre::eyre::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tokio::select;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::tui::FrameRequester;
use crate::vt100_render::render_screen;

const CURSOR_POSITION_REQUEST: &[u8] = b"\x1b[6n";
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
const TERMINAL_SCROLLBACK: usize = 2_048;

struct SharedTerminalState {
    parser: vt100::Parser,
    exit_code: Option<i32>,
}

struct ForkSessionTerminalIo {
    session: ProcessHandle,
    writer_tx: mpsc::Sender<Vec<u8>>,
    update_task: JoinHandle<()>,
}

pub(crate) struct ForkSessionTerminal {
    shared: Arc<Mutex<SharedTerminalState>>,
    io: Option<ForkSessionTerminalIo>,
    last_size: Option<TerminalSize>,
}

impl ForkSessionTerminal {
    pub(crate) async fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        env: HashMap<String, String>,
        size: TerminalSize,
        frame_requester: FrameRequester,
    ) -> Result<Self> {
        let SpawnedProcess {
            session,
            stdout_rx,
            stderr_rx,
            exit_rx,
        } = codex_utils_pty::spawn_pty_process(program, args, cwd, &env, &None, size)
            .await
            .map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;
        let writer_tx = session.writer_sender();
        let shared = Arc::new(Mutex::new(SharedTerminalState {
            parser: vt100::Parser::new(size.rows, size.cols, TERMINAL_SCROLLBACK),
            exit_code: None,
        }));
        let update_task = spawn_terminal_update_task(
            shared.clone(),
            stdout_rx,
            stderr_rx,
            exit_rx,
            writer_tx.clone(),
            frame_requester,
        );

        Ok(Self {
            shared,
            io: Some(ForkSessionTerminalIo {
                session,
                writer_tx,
                update_task,
            }),
            last_size: Some(size),
        })
    }

    pub(crate) fn resize(&mut self, size: TerminalSize) {
        if self.last_size == Some(size) {
            return;
        }

        if let Ok(mut shared) = self.shared.lock() {
            shared.parser.screen_mut().set_size(size.rows, size.cols);
        }
        if let Some(io) = self.io.as_ref() {
            let _ = io.session.resize(size);
        }
        self.last_size = Some(size);
    }

    pub(crate) async fn handle_key_event(&self, key_event: KeyEvent) -> bool {
        let Some(io) = self.io.as_ref() else {
            return false;
        };
        if self.exit_code().is_some() {
            return false;
        }
        let application_cursor = self.application_cursor();
        let Some(bytes) = encode_key_event(key_event, application_cursor) else {
            return false;
        };
        io.writer_tx.send(bytes).await.is_ok()
    }

    pub(crate) async fn handle_paste(&self, pasted: &str) -> bool {
        let Some(io) = self.io.as_ref() else {
            return false;
        };
        if self.exit_code().is_some() {
            return false;
        }
        let bytes = {
            let bracketed_paste = self.bracketed_paste();
            encode_paste(bracketed_paste, pasted)
        };
        io.writer_tx.send(bytes).await.is_ok()
    }

    pub(crate) fn exit_code(&self) -> Option<i32> {
        self.shared.lock().ok().and_then(|shared| shared.exit_code)
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
        if area.is_empty() {
            return None;
        }

        let shared = self.shared.lock().ok()?;
        render_screen(shared.parser.screen(), area, buf)
    }

    pub(crate) fn terminate(&mut self) {
        if let Some(io) = self.io.take() {
            io.update_task.abort();
            io.session.terminate();
        }
    }

    fn application_cursor(&self) -> bool {
        self.shared
            .lock()
            .ok()
            .is_some_and(|shared| shared.parser.screen().application_cursor())
    }

    fn bracketed_paste(&self) -> bool {
        self.shared
            .lock()
            .ok()
            .is_some_and(|shared| shared.parser.screen().bracketed_paste())
    }
}

impl Drop for ForkSessionTerminal {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn spawn_terminal_update_task(
    shared: Arc<Mutex<SharedTerminalState>>,
    mut stdout_rx: mpsc::Receiver<Vec<u8>>,
    mut stderr_rx: mpsc::Receiver<Vec<u8>>,
    mut exit_rx: tokio::sync::oneshot::Receiver<i32>,
    writer_tx: mpsc::Sender<Vec<u8>>,
    frame_requester: FrameRequester,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut exit_open = true;
        let mut request_tail = Vec::new();

        loop {
            select! {
                stdout = stdout_rx.recv(), if stdout_open => match stdout {
                    Some(chunk) => {
                        process_output_chunk(
                            &shared,
                            &writer_tx,
                            &frame_requester,
                            &mut request_tail,
                            chunk,
                        ).await;
                    }
                    None => {
                        stdout_open = false;
                    }
                },
                stderr = stderr_rx.recv(), if stderr_open => match stderr {
                    Some(chunk) => {
                        process_output_chunk(
                            &shared,
                            &writer_tx,
                            &frame_requester,
                            &mut request_tail,
                            chunk,
                        ).await;
                    }
                    None => {
                        stderr_open = false;
                    }
                },
                exit = &mut exit_rx, if exit_open => {
                    let exit_code = exit.unwrap_or(-1);
                    if let Ok(mut shared) = shared.lock() {
                        shared.exit_code = Some(exit_code);
                    }
                    exit_open = false;
                    frame_requester.schedule_frame();
                }
                else => break,
            }

            if !stdout_open && !stderr_open && !exit_open {
                break;
            }
        }
    })
}

async fn process_output_chunk(
    shared: &Arc<Mutex<SharedTerminalState>>,
    writer_tx: &mpsc::Sender<Vec<u8>>,
    frame_requester: &FrameRequester,
    request_tail: &mut Vec<u8>,
    chunk: Vec<u8>,
) {
    let request_count = count_cursor_position_requests(request_tail, &chunk);
    let responses = if let Ok(mut shared) = shared.lock() {
        shared.parser.process(&chunk);
        let (row, col) = shared.parser.screen().cursor_position();
        vec![cursor_position_response(row, col); request_count]
    } else {
        Vec::new()
    };

    for response in responses {
        if writer_tx.send(response).await.is_err() {
            break;
        }
    }
    frame_requester.schedule_frame();
}

fn count_cursor_position_requests(request_tail: &mut Vec<u8>, chunk: &[u8]) -> usize {
    let mut combined = Vec::with_capacity(request_tail.len() + chunk.len());
    combined.extend_from_slice(request_tail);
    combined.extend_from_slice(chunk);

    let request_count = combined
        .windows(CURSOR_POSITION_REQUEST.len())
        .filter(|window| *window == CURSOR_POSITION_REQUEST)
        .count();

    let keep = CURSOR_POSITION_REQUEST.len().saturating_sub(1);
    if combined.len() > keep {
        request_tail.clear();
        request_tail.extend_from_slice(&combined[combined.len() - keep..]);
    } else {
        request_tail.clear();
        request_tail.extend_from_slice(&combined);
    }

    request_count
}

fn cursor_position_response(row: u16, col: u16) -> Vec<u8> {
    format!("\x1b[{};{}R", row + 1, col + 1).into_bytes()
}

fn encode_paste(bracketed_paste: bool, pasted: &str) -> Vec<u8> {
    if !bracketed_paste {
        return pasted.as_bytes().to_vec();
    }

    let mut bytes =
        Vec::with_capacity(BRACKETED_PASTE_START.len() + pasted.len() + BRACKETED_PASTE_END.len());
    bytes.extend_from_slice(BRACKETED_PASTE_START);
    bytes.extend_from_slice(pasted.as_bytes());
    bytes.extend_from_slice(BRACKETED_PASTE_END);
    bytes
}

fn encode_key_event(key_event: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    if !matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }

    let modifiers = key_event.modifiers;
    let alt = modifiers.contains(KeyModifiers::ALT) && !crate::key_hint::is_altgr(modifiers);
    let control =
        modifiers.contains(KeyModifiers::CONTROL) && !crate::key_hint::is_altgr(modifiers);

    let mut bytes = match key_event.code {
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Left if alt => b"\x1bb".to_vec(),
        KeyCode::Right if alt => b"\x1bf".to_vec(),
        KeyCode::Left => {
            if application_cursor {
                b"\x1bOD".to_vec()
            } else {
                b"\x1b[D".to_vec()
            }
        }
        KeyCode::Right => {
            if application_cursor {
                b"\x1bOC".to_vec()
            } else {
                b"\x1b[C".to_vec()
            }
        }
        KeyCode::Up => {
            if application_cursor {
                b"\x1bOA".to_vec()
            } else {
                b"\x1b[A".to_vec()
            }
        }
        KeyCode::Down => {
            if application_cursor {
                b"\x1bOB".to_vec()
            } else {
                b"\x1b[B".to_vec()
            }
        }
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Char(c) => {
            if control && let Some(control_byte) = encode_control_char(c) {
                vec![control_byte]
            } else {
                c.to_string().into_bytes()
            }
        }
        _ => return None,
    };

    if alt && !matches!(key_event.code, KeyCode::Left | KeyCode::Right) {
        let mut prefixed = Vec::with_capacity(bytes.len() + 1);
        prefixed.push(0x1b);
        prefixed.extend_from_slice(&bytes);
        bytes = prefixed;
    }

    Some(bytes)
}

fn encode_control_char(c: char) -> Option<u8> {
    match c {
        'a' | 'A' => Some(0x01),
        'b' | 'B' => Some(0x02),
        'c' | 'C' => Some(0x03),
        'd' | 'D' => Some(0x04),
        'e' | 'E' => Some(0x05),
        'f' | 'F' => Some(0x06),
        'g' | 'G' => Some(0x07),
        'h' | 'H' => Some(0x08),
        'i' | 'I' => Some(0x09),
        'j' | 'J' => Some(0x0a),
        'k' | 'K' => Some(0x0b),
        'l' | 'L' => Some(0x0c),
        'm' | 'M' => Some(0x0d),
        'n' | 'N' => Some(0x0e),
        'o' | 'O' => Some(0x0f),
        'p' | 'P' => Some(0x10),
        'q' | 'Q' => Some(0x11),
        'r' | 'R' => Some(0x12),
        's' | 'S' => Some(0x13),
        't' | 'T' => Some(0x14),
        'u' | 'U' => Some(0x15),
        'v' | 'V' => Some(0x16),
        'w' | 'W' => Some(0x17),
        'x' | 'X' => Some(0x18),
        'y' | 'Y' => Some(0x19),
        'z' | 'Z' => Some(0x1a),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        ' ' => Some(0x00),
        _ => None,
    }
}

#[cfg(test)]
impl ForkSessionTerminal {
    pub(crate) fn for_test(parser: vt100::Parser, exit_code: Option<i32>) -> Self {
        Self {
            shared: Arc::new(Mutex::new(SharedTerminalState { parser, exit_code })),
            io: None,
            last_size: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn ctrl_char_maps_to_control_byte() {
        assert_eq!(
            encode_key_event(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                /*application_cursor*/ false
            ),
            Some(vec![0x03])
        );
    }

    #[test]
    fn alt_left_uses_word_motion_fallback() {
        assert_eq!(
            encode_key_event(
                KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
                /*application_cursor*/ false,
            ),
            Some(b"\x1bb".to_vec())
        );
    }

    #[test]
    fn bracketed_paste_wraps_contents() {
        assert_eq!(
            encode_paste(/*bracketed_paste*/ true, "hello"),
            b"\x1b[200~hello\x1b[201~".to_vec()
        );
    }

    #[test]
    fn cursor_position_requests_detect_across_chunk_boundaries() {
        let mut request_tail = Vec::new();
        assert_eq!(
            count_cursor_position_requests(&mut request_tail, b"\x1b["),
            0
        );
        assert_eq!(count_cursor_position_requests(&mut request_tail, b"6n"), 1);
    }
}
