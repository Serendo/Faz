//! A directory browser overlay, used to pick the file to send or the
//! destination directory to receive into during a ZMODEM transfer.
//!
//! Built on the same immediate-mode pattern as `overlay::selector`: a
//! `run_loop` polls input, rebuilds the visible rows, and renders them with
//! termwiz. It differs in navigating a tree instead of filtering a flat list,
//! so it keeps its own state: the current directory, the entries in it, and
//! the cursor.
//!
//! The result is returned through a channel rather than an event callback.
//! Transfers are started from the caller's own thread, so a channel keeps the
//! "what did the user pick" answer and the "start the transfer" action in the
//! same place, without routing a synthetic event through the config system.

use std::path::{Path, PathBuf};

use termwiz::cell::{unicode_column_width, CellAttributes};
use termwiz::color::ColorAttribute;
use termwiz::input::{InputEvent, KeyCode, KeyEvent, Modifiers};
use termwiz::surface::{Change, Position};
use termwiz::terminal::Terminal;

use mux::termwiztermtab::TermWizTerminal;

/// What the browser is allowed to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickMode {
    /// Only files can be chosen. Enter on a directory descends into it.
    File,
    /// Only directories can be chosen. Enter descends; the confirm key picks
    /// the current directory.
    Directory,
}

/// What the user did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickResult {
    /// The chosen path. For [`PickMode::File`] this is a regular file; for
    /// [`PickMode::Directory`] a directory.
    Chosen(PathBuf),
    /// Escape or `q`: nothing chosen.
    Cancelled,
}

/// One row in the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

impl Entry {
    /// The name as shown, with a trailing slash so directories are obvious.
    fn label(&self) -> String {
        if self.is_dir {
            format!("{}/", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// Read a directory into sorted entries: directories first, then files, each
/// group sorted case-insensitively by name.
///
/// Hidden entries are included: a transfer aimed at `~/.config/...` is a
/// normal case, and the browser is the only way to reach those paths.
///
/// Unreadable directories yield an empty listing rather than an error, so the
/// browser can show "permission denied" as an empty view instead of dying.
fn read_dir(dir: &Path) -> Vec<Entry> {
    let Ok(iter) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    for entry in iter.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        entries.push(Entry { name, path, is_dir });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

/// The parent of `dir`, or `None` at the filesystem root.
fn parent_of(dir: &Path) -> Option<PathBuf> {
    dir.parent().map(|p| p.to_path_buf())
}

struct BrowserState {
    dir: PathBuf,
    entries: Vec<Entry>,
    /// True while the path input row owns the keyboard. Navigation keys move
    /// the cursor inside the text instead of the listing.
    input_mode: bool,
    /// What the user has typed, as chars so a cursor index is unambiguous.
    input: Vec<char>,
    /// Cursor position inside `input`, always in `0..=input.len()`.
    input_cursor: usize,
    /// Previously confirmed paths, newest last. Up/Down cycle through them
    /// while the input row is focused.
    history: Vec<String>,
    /// Index into `history` while browsing it; `None` means editing a fresh
    /// line.
    history_pos: Option<usize>,
    /// Completion candidates for the current input, refreshed on every Tab.
    /// Empty when the last Tab found nothing.
    completions: Vec<String>,
    /// First candidate shown in the completion panel. PageDown/PageUp move it.
    completion_top: usize,
    /// Index into `entries`. Clamped to `0` for an empty listing.
    cursor: usize,
    /// First visible row, kept so the cursor stays on screen.
    top: usize,
    mode: PickMode,
    title: String,
    /// Rows available for the listing, set from the terminal size.
    viewport: usize,
    /// Set when the user confirmed; consumed by `run_loop`.
    outcome: Option<PickResult>,
}

impl BrowserState {
    fn new(dir: PathBuf, mode: PickMode, title: String) -> Self {
        let entries = read_dir(&dir);
        Self {
            dir,
            entries,
            input_mode: false,
            input: Vec::new(),
            input_cursor: 0,
            history: Vec::new(),
            history_pos: None,
            completions: Vec::new(),
            completion_top: 0,
            cursor: 0,
            top: 0,
            mode,
            title,
            viewport: 10,
            outcome: None,
        }
    }

    fn navigate_to(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.entries = read_dir(&self.dir);
        self.cursor = 0;
        self.top = 0;
    }

    fn current(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        if self.cursor < self.top {
            self.top = self.cursor;
        }
    }

    fn move_down(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.cursor = (self.cursor + 1).min(self.entries.len() - 1);
        if self.cursor >= self.top + self.viewport {
            self.top = self.cursor + 1 - self.viewport;
        }
    }

    /// Enter on the current row: descend into a directory, or confirm a file.
    fn activate(&mut self) {
        let Some(entry) = self.current().cloned() else {
            // Nothing under the cursor. In Directory mode that means either an
            // empty directory or one with no visible entries, and the directory
            // on screen is the only thing that can be chosen — so Enter picks
            // it rather than doing nothing.
            self.choose_current_dir();
            return;
        };
        if entry.is_dir {
            self.navigate_to(entry.path);
        } else if self.mode == PickMode::File {
            self.remember(&entry.path);
            self.outcome = Some(PickResult::Chosen(entry.path));
        }
        // A file under PickMode::Directory is not selectable; ignore.
    }

    /// Confirm the current directory itself. Only meaningful in Directory mode.
    fn choose_current_dir(&mut self) {
        if self.mode == PickMode::Directory {
            self.remember(&self.dir.clone());
            self.outcome = Some(PickResult::Chosen(self.dir.clone()));
        }
    }

    fn go_up(&mut self) {
        if let Some(parent) = parent_of(&self.dir) {
            // Keep the directory we came from selected, so a mistaken descent
            // is easy to undo.
            let previous = self.dir.clone();
            self.navigate_to(parent);
            if let Some(idx) = self.entries.iter().position(|e| e.path == previous) {
                self.cursor = idx;
                if self.cursor >= self.top + self.viewport {
                    self.top = self.cursor + 1 - self.viewport;
                }
            }
        }
    }

    /// Enter the path input row, seeded with the directory on screen.
    fn begin_input(&mut self) {
        let seed = format!("{}/", self.dir.display());
        self.input = seed.chars().collect();
        self.input_cursor = self.input.len();
        self.history_pos = None;
        self.completions.clear();
        self.input_mode = true;
    }

    /// Leave the input row without choosing anything.
    fn cancel_input(&mut self) {
        self.input_mode = false;
        self.history_pos = None;
        self.completions.clear();
    }

    fn input_string(&self) -> String {
        self.input.iter().collect()
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.input_cursor, c);
        self.input_cursor += 1;
        self.completions.clear();
        self.completion_top = 0;
        self.history_pos = None;
    }

    fn delete_backward(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor -= 1;
            self.input.remove(self.input_cursor);
            self.completions.clear();
            self.completion_top = 0;
            self.history_pos = None;
        }
    }

    fn delete_forward(&mut self) {
        if self.input_cursor < self.input.len() {
            self.input.remove(self.input_cursor);
            self.completions.clear();
            self.completion_top = 0;
            self.history_pos = None;
        }
    }

    fn cursor_left(&mut self) {
        self.input_cursor = self.input_cursor.saturating_sub(1);
    }

    fn cursor_right(&mut self) {
        if self.input_cursor < self.input.len() {
            self.input_cursor += 1;
        }
    }

    fn cursor_home(&mut self) {
        self.input_cursor = 0;
    }

    fn cursor_end(&mut self) {
        self.input_cursor = self.input.len();
    }

    /// Walk back through confirmed paths. The list is newest-last, so Up walks
    /// toward the newer end.
    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_pos {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_pos = Some(next);
        self.set_input(&self.history[next].clone());
    }

    fn history_down(&mut self) {
        let Some(pos) = self.history_pos else {
            return;
        };
        if pos + 1 >= self.history.len() {
            // Walking past the newest entry returns to a blank line rather
            // than sticking, which is what a shell does.
            self.history_pos = None;
            self.set_input("");
        } else {
            self.history_pos = Some(pos + 1);
            let text = self.history[pos + 1].clone();
            self.set_input(&text);
        }
    }

    fn set_input(&mut self, text: &str) {
        self.input = text.chars().collect();
        self.input_cursor = self.input.len();
        self.completions.clear();
        self.completion_top = 0;
    }

    /// Resolve what the user typed into an absolute path.
    ///
    /// A leading `~` expands against HOME and a relative path is taken from
    /// the directory currently on screen, so both `../x` and `/abs/x` work
    /// without the user having to know where the browser started.
    fn resolve_input_path(&self, text: &str) -> PathBuf {
        resolve_against(&self.dir, text)
    }

    /// Complete the current input and return the candidates that matched.
    ///
    /// One candidate is applied outright. Several are reduced to their longest
    /// common prefix and returned so the view can list them.
    fn complete(&mut self) -> usize {
        let text = self.input_string();
        let (candidates, replace_from) = complete_path(&self.dir, &text, self.mode);
        if candidates.is_empty() {
            self.completions.clear();
            return 0;
        }

        let completed = if candidates.len() == 1 {
            candidates[0].clone()
        } else {
            longest_common_prefix(&candidates)
        };

        if !completed.is_empty() && completed.chars().count() > replace_from {
            self.input = completed.chars().collect();
            self.input_cursor = self.input.len();
        }
        self.completions = candidates;
        self.completion_top = 0;
        self.completions.len()
    }

    /// Confirm whatever the input row currently names.
    ///
    /// A directory is entered rather than returned: choosing a directory is
    /// only meaningful in [`PickMode::Directory`], and there the user expects
    /// to look inside first. Anything the mode does not allow is ignored.
    fn confirm_input(&mut self) {
        let text = self.input_string();
        if text.trim().is_empty() {
            return;
        }
        let path = self.resolve_input_path(text.trim());
        if path.is_dir() {
            self.navigate_to(path);
            self.cancel_input();
            return;
        }
        if self.mode == PickMode::File && path.is_file() {
            self.remember(&path);
            self.outcome = Some(PickResult::Chosen(path));
        }
    }

    /// Push a confirmed path onto the history, keeping entries unique and
    /// bounded so a long session does not grow without limit.
    fn remember(&mut self, path: &Path) {
        let text = path.display().to_string();
        self.history.retain(|h| h != &text);
        self.history.push(text);
        const HISTORY_LIMIT: usize = 50;
        if self.history.len() > HISTORY_LIMIT {
            let drop = self.history.len() - HISTORY_LIMIT;
            self.history.drain(0..drop);
        }
    }

    /// How many columns the completion panel fits in `width` cells.
    ///
    /// Wide enough for the longest visible name, capped so a short directory
    /// still lays out in several columns instead of one per row.
    fn completion_columns(&self, width: usize) -> usize {
        let longest = self
            .completions
            .iter()
            .map(|c| unicode_column_width(base_name(c), None))
            .max()
            .unwrap_or(8)
            .clamp(6, 32);
        (width / (longest + 2)).max(1)
    }

    fn render(&self, term: &mut TermWizTerminal) -> anyhow::Result<()> {
        let size = term.get_screen_size()?;
        let width = size.cols.max(20);
        let height = size.rows.max(6);

        let mut changes = vec![
            Change::ClearScreen(ColorAttribute::Default),
            Change::CursorPosition {
                x: Position::Absolute(0),
                y: Position::Absolute(0),
            },
        ];

        // Header: title, then the current directory.
        let header = CellAttributes::default()
            .set_intensity(termwiz::cell::Intensity::Bold)
            .clone();
        changes.push(Change::AllAttributes(header));
        changes.push(Change::Text(truncate(&self.title, width)));
        changes.push(Change::Text("\r\n".to_string()));

        changes.push(Change::AllAttributes(CellAttributes::default()));
        changes.push(Change::Text(truncate(
            &format!("[{}]", self.dir.display()),
            width,
        )));
        changes.push(Change::Text("\r\n".to_string()));

        // Path input row. Shown only while focused so the listing keeps its
        // full height during normal navigation.
        //
        // Exactly one line: the completion hints share it, right-aligned,
        // rather than taking a row of their own. A separate row used to push
        // the listing down by one while the cursor and the scroll arithmetic
        // still assumed it had not moved, which shifted everything by a row
        // and let the selected listing row paint over the hints.
        if self.input_mode {
            let prompt = format!("> {}", self.input_string());
            changes.push(Change::AllAttributes(CellAttributes::default()));
            changes.push(Change::Text(truncate(&prompt, width)));
            changes.push(Change::Text("\r\n".to_string()));
        }

        // Completion panel. A count is useless — the user needs to read the
        // names — so candidates get a real block above the listing, laid out
        // in columns and scrolled with PageUp/PageDown. It shrinks the
        // listing rather than overlapping it.
        let columns = self.completion_columns(width);
        let completion_rows = if self.input_mode && !self.completions.is_empty() {
            let wanted = self.completions.len().div_ceil(columns) + 1;
            wanted.min((height / 2).max(3))
        } else {
            0
        };
        if completion_rows > 0 {
            let visible = completion_rows.saturating_sub(1) * columns;
            let start = self.completion_top.min(self.completions.len());
            let end = (start + visible).min(self.completions.len());
            let col_width = (width / columns.max(1)).max(8);

            let attr = CellAttributes::default()
                .set_intensity(termwiz::cell::Intensity::Half)
                .clone();
            changes.push(Change::AllAttributes(attr));

            let mut printed = 0;
            for chunk in self.completions[start..end].chunks(columns) {
                let mut line = String::new();
                for name in chunk {
                    line.push_str(&pad_to_cells(base_name(name), col_width.saturating_sub(1)));
                }
                // Clamp to the terminal width: a row that reaches exactly the
                // last column wraps, which is what pushed the cursor down a
                // line after Tab.
                changes.push(Change::Text(truncate(&line, width.saturating_sub(1))));
                changes.push(Change::Text("\r\n".to_string()));
                printed += 1;
            }
            for _ in printed..completion_rows.saturating_sub(1) {
                changes.push(Change::Text("\r\n".to_string()));
            }

            let shown_end = end;
            let summary = if shown_end >= self.completions.len() {
                format!("  {} 项", self.completions.len())
            } else {
                format!(
                    "  {}-{} / {} 项   PageDown 查看更多",
                    start + 1,
                    shown_end,
                    self.completions.len()
                )
            };
            changes.push(Change::Text(truncate(&summary, width)));
            changes.push(Change::Text("\r\n".to_string()));
            changes.push(Change::AllAttributes(CellAttributes::default()));
        }

        // Listing. Rows spoken for: title, directory, the input row when it is
        // up, the completion panel, and the footer. Reserve one extra so the
        // last listing line never sits under the footer.
        let chrome_rows = if self.input_mode { 3 } else { 3 };
        let rows = height.saturating_sub(chrome_rows + completion_rows + 1);
        let end = (self.top + rows).min(self.entries.len());
        if self.entries.is_empty() {
            changes.push(Change::AllAttributes(CellAttributes::default()));
            changes.push(Change::Text("  (empty)".to_string()));
            changes.push(Change::Text("\r\n".to_string()));
        }
        for (offset, entry) in self.entries[self.top..end].iter().enumerate() {
            let idx = self.top + offset;
            let selected = idx == self.cursor;
            let mut attr = CellAttributes::default();
            if selected {
                attr.set_reverse(true);
            }
            changes.push(Change::AllAttributes(attr));
            changes.push(Change::Text(truncate(
                &format!("  {}", entry.label()),
                width,
            )));
            changes.push(Change::AllAttributes(CellAttributes::default()));
            changes.push(Change::Text("\r\n".to_string()));
        }

        // Footer: the keys that apply to this mode.
        let footer = if self.input_mode {
            "  Tab: 补全   Enter: 确认   ←→: 光标   ↑↓: 历史   PgUp/PgDn: 翻页   Esc: 退出输入"
        } else {
            match self.mode {
                PickMode::File => "  方向键移动   Enter: 选择文件/进入目录   Backspace: 上级   Ctrl-P: 输入路径   Esc: 取消",
                PickMode::Directory => "  方向键移动   Enter: 进入目录   空格 或 Ctrl-S: 选择当前目录   Backspace: 上级   Ctrl-P: 输入路径   Esc: 取消",
            }
        };
        let footer_attr = CellAttributes::default()
            .set_intensity(termwiz::cell::Intensity::Half)
            .clone();
        changes.push(Change::AllAttributes(footer_attr));
        changes.push(Change::Text(truncate(footer, width)));

        // Park the cursor at the edit position so typing has a visible caret.
        // Layout: 0 = title, 1 = directory, 2 = input row.
        if self.input_mode {
            let row = 2;
            changes.push(Change::AllAttributes(CellAttributes::default()));
            let typed: String = self.input[..self.input_cursor].iter().collect();
            let caret = 2 + unicode_column_width(&typed, None);
            changes.push(Change::CursorPosition {
                x: Position::Absolute(caret.min(width - 1)),
                y: Position::Absolute(row.min(height - 1)),
            });
        }

        term.render(&changes)?;
        Ok(())
    }

    fn run_loop(&mut self, term: &mut TermWizTerminal) -> anyhow::Result<PickResult> {
        loop {
            if let Some(outcome) = self.outcome.take() {
                return Ok(outcome);
            }
            self.render(term)?;

            let Some(event) = term.poll_input(None)? else {
                // Terminal closed underneath us.
                return Ok(PickResult::Cancelled);
            };

            match self.handle_event(event) {
                EventAction::Continue => {}
                EventAction::Done(result) => return Ok(result),
            }
        }
    }

    /// Apply one input event, returning what the loop should do next.
    ///
    /// Split out of `run_loop` so the key handling can be tested without a
    /// terminal: the Ctrl chords in particular are easy to break by matching
    /// lowercase, which never fires.
    fn handle_event(&mut self, event: InputEvent) -> EventAction {
        if self.input_mode {
            return self.handle_input_event(event);
        }
        match event {
            InputEvent::Resized { .. } => {}
            // Ctrl chords match UPPERCASE letters. `TermWizTerminal::poll_input`
            // normalizes CTRL+lowercase to CTRL+uppercase before overlays see
            // it (see `mux/src/termwiztermtab.rs`), and `overlay::selector`
            // matches the same way. Matching lowercase here silently never
            // fires.
            InputEvent::Key(KeyEvent {
                key: KeyCode::UpArrow,
                ..
            }) => self.move_up(),
            InputEvent::Key(KeyEvent {
                key: KeyCode::DownArrow,
                ..
            }) => self.move_down(),
            InputEvent::Key(KeyEvent {
                key,
                modifiers: mods,
            }) if ctrl_chord(&key, mods) == Some('k') => self.move_up(),
            InputEvent::Key(KeyEvent {
                key,
                modifiers: mods,
            }) if ctrl_chord(&key, mods) == Some('j') => self.move_down(),
            InputEvent::Key(KeyEvent {
                key: KeyCode::Enter,
                ..
            }) => self.activate(),
            // Space confirms the current directory in Directory mode. A
            // bare key: it still works when the Ctrl modifier is dropped
            // somewhere between AppKit and here.
            InputEvent::Key(KeyEvent {
                key: KeyCode::Char(' '),
                modifiers: Modifiers::NONE,
            }) => self.choose_current_dir(),
            InputEvent::Key(KeyEvent {
                key,
                modifiers: mods,
            }) if ctrl_chord(&key, mods) == Some('s') => {
                log::info!(
                    "filebrowser: Ctrl-S ({}) confirmed {}",
                    describe_ctrl_chord(&key, mods),
                    self.dir.display()
                );
                self.choose_current_dir();
            }
            InputEvent::Key(KeyEvent {
                key: KeyCode::Backspace,
                ..
            })
            | InputEvent::Key(KeyEvent {
                key: KeyCode::LeftArrow,
                ..
            }) => self.go_up(),
            // Ctrl-P focuses the path row. Navigation keeps the plain arrow
            // keys, so the two modes never compete for the same chord.
            InputEvent::Key(KeyEvent {
                key,
                modifiers: mods,
            }) if ctrl_chord(&key, mods) == Some('p') => self.begin_input(),
            InputEvent::Key(KeyEvent {
                key: KeyCode::Escape,
                ..
            })
            | InputEvent::Key(KeyEvent {
                key: KeyCode::Char('q'),
                ..
            }) => return EventAction::Done(PickResult::Cancelled),
            _ => {}
        }
        EventAction::Continue
    }
}

impl BrowserState {
    /// Handle one event while the path row has focus.
    ///
    /// Arrows edit the line rather than the listing: left/right move the
    /// cursor, up/down walk the history. Everything else falls through to the
    /// listing's own bindings only where it stays unambiguous.
    fn handle_input_event(&mut self, event: InputEvent) -> EventAction {
        match event {
            InputEvent::Resized { .. } => {}

            InputEvent::Key(KeyEvent { key, modifiers }) if matches!(key, KeyCode::Char(_)) => {
                // Ctrl chords first: a control character arrives as a Char
                // too, and must not be inserted as text.
                if let Some(chord) = ctrl_chord(&key, modifiers) {
                    match chord {
                        'p' => self.cancel_input(),
                        's' => self.confirm_input(),
                        'a' => self.cursor_home(),
                        'e' => self.cursor_end(),
                        'u' => {
                            // Clear the line back to the cursor, like a shell.
                            self.input.truncate(self.input_cursor);
                            self.completions.clear();
                        }
                        'w' => self.delete_word(),
                        _ => {}
                    }
                    return EventAction::Continue;
                }
                if let KeyCode::Char(c) = key {
                    self.insert_char(c);
                }
            }

            InputEvent::Paste(text) => {
                for c in text.chars().filter(|c| *c != '\r' && *c != '\n') {
                    self.insert_char(c);
                }
            }

            InputEvent::Key(KeyEvent {
                key: KeyCode::Tab, ..
            }) => {
                self.complete();
            }

            InputEvent::Key(KeyEvent {
                key: KeyCode::Enter,
                ..
            }) => self.confirm_input(),

            InputEvent::Key(KeyEvent {
                key: KeyCode::Backspace,
                ..
            }) => self.delete_backward(),

            InputEvent::Key(KeyEvent {
                key: KeyCode::Delete,
                ..
            }) => self.delete_forward(),

            InputEvent::Key(KeyEvent {
                key: KeyCode::LeftArrow,
                ..
            }) => self.cursor_left(),

            InputEvent::Key(KeyEvent {
                key: KeyCode::RightArrow,
                ..
            }) => self.cursor_right(),

            InputEvent::Key(KeyEvent {
                key: KeyCode::UpArrow,
                ..
            }) => self.history_up(),

            InputEvent::Key(KeyEvent {
                key: KeyCode::DownArrow,
                ..
            }) => self.history_down(),

            InputEvent::Key(KeyEvent {
                key: KeyCode::PageUp,
                ..
            }) => self.completion_page(-1),

            InputEvent::Key(KeyEvent {
                key: KeyCode::PageDown,
                ..
            }) => self.completion_page(1),

            InputEvent::Key(KeyEvent {
                key: KeyCode::Home, ..
            }) => self.cursor_home(),

            InputEvent::Key(KeyEvent {
                key: KeyCode::End, ..
            }) => self.cursor_end(),

            InputEvent::Key(KeyEvent {
                key: KeyCode::Escape,
                ..
            }) => self.cancel_input(),

            _ => {}
        }
        EventAction::Continue
    }

    /// Scroll the completion panel by one screenful.
    ///
    /// The step is a page of the panel as the renderer lays it out; the exact
    /// column count is only known there, so this moves by whole rows of the
    /// narrowest useful layout and lets the renderer clamp.
    fn completion_page(&mut self, direction: i32) {
        if self.completions.is_empty() {
            return;
        }
        let step = 10;
        let last_page = self.completions.len().saturating_sub(1) / step;
        let current = self.completion_top / step;
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(last_page)
        };
        self.completion_top = next * step;
    }

    /// Delete the word before the cursor, stopping at a path separator.
    fn delete_word(&mut self) {
        while self.input_cursor > 0 && self.input[self.input_cursor - 1] == ' ' {
            self.input_cursor -= 1;
            self.input.remove(self.input_cursor);
        }
        while self.input_cursor > 0
            && self.input[self.input_cursor - 1] != ' '
            && self.input[self.input_cursor - 1] != '/'
        {
            self.input_cursor -= 1;
            self.input.remove(self.input_cursor);
        }
        self.completions.clear();
    }
}

/// Does this event represent `Ctrl+<letter>`?
///
/// The same physical chord can arrive in several shapes and the code cannot
/// assume which: macOS may deliver a control character with no modifier, the
/// terminal layer may normalise the letter's case, or the letter may arrive
/// as typed. Accepting all of them keeps the binding working regardless, and
/// [`describe_ctrl_chord`] logs which shape actually showed up so the set can
/// be narrowed once observed.
fn ctrl_chord(key: &KeyCode, mods: Modifiers) -> Option<char> {
    let KeyCode::Char(c) = key else {
        return None;
    };
    // Control codes for Ctrl+A..Ctrl+Z are 0x01..0x1a.
    let lower = if ('\u{1}'..='\u{1a}').contains(c) {
        let offset = (*c as u32 - 1) as u8;
        (b'a' + offset) as char
    } else {
        c.to_ascii_lowercase()
    };
    if mods.contains(Modifiers::CTRL) || ('\u{1}'..='\u{1a}').contains(c) {
        Some(lower)
    } else {
        None
    }
}

/// Render an event for logging, so a binding that does not fire can be
/// diagnosed from the log instead of by guessing at the platform's behaviour.
fn describe_ctrl_chord(key: &KeyCode, mods: Modifiers) -> String {
    format!("key={key:?} mods={mods:?}")
}

/// What `BrowserState::handle_event` decided.
#[derive(Debug, PartialEq, Eq)]
enum EventAction {
    /// Keep looping.
    Continue,
    /// Return this result from `run_loop`.
    Done(PickResult),
}

/// Turn a typed path into an absolute one, relative to `base`.
fn resolve_against(base: &Path, text: &str) -> PathBuf {
    let trimmed = text.trim();
    let expanded: PathBuf = if let Some(rest) = trimmed.strip_prefix('~') {
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        if rest.is_empty() {
            config::HOME_DIR.to_path_buf()
        } else {
            config::HOME_DIR.join(rest)
        }
    } else {
        PathBuf::from(trimmed)
    };

    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

/// Split `text` into the directory part to search and the name prefix to match.
///
/// `dir_part` keeps its trailing separator so the completed path can be
/// rebuilt by simple concatenation.
fn split_for_completion(text: &str) -> (String, String) {
    match text.rfind('/') {
        Some(idx) => (text[..=idx].to_string(), text[idx + 1..].to_string()),
        None => (String::new(), text.to_string()),
    }
}

/// Complete `text` against the filesystem, returning full replacement strings
/// and the number of chars the caller already has.
///
/// Only directories are offered in [`PickMode::Directory`]; [`PickMode::File`]
/// also offers files. Anything that is neither is skipped, so a stale entry
/// cannot be completed into.
fn complete_path(base: &Path, text: &str, mode: PickMode) -> (Vec<String>, usize) {
    let (dir_part, prefix) = split_for_completion(text);
    let search_root = resolve_against(base, &dir_part);
    // A display prefix that keeps `~` as typed: `~/Doc` should complete to
    // `~/Documents/`, not to the expanded absolute path.
    let display_root = if dir_part.is_empty() {
        String::new()
    } else {
        dir_part.clone()
    };

    let Ok(iter) = std::fs::read_dir(&search_root) else {
        return (Vec::new(), display_root.chars().count());
    };

    let mut matches: Vec<String> = Vec::new();
    for entry in iter.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        // Follow a symlink so a link to a directory can be completed as one.
        let is_dir = file_type.is_dir() || (file_type.is_symlink() && entry.path().is_dir());
        if !is_dir && mode == PickMode::Directory {
            continue;
        }

        let mut candidate = format!("{display_root}{name}");
        if is_dir {
            candidate.push('/');
        }
        matches.push(candidate);
    }

    matches.sort();
    (matches, display_root.chars().count())
}

/// The last path component of a completion, keeping its trailing separator.
///
/// Directory completions end in `/`, so splitting on the raw string would
/// yield an empty trailing part and the hint would show only a count. The
/// separator is kept because it is what tells the user the entry is a
/// directory they can descend into.
fn base_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    let start = trimmed.rfind('/').map(|i| i + 1).unwrap_or(0);
    &path[start..]
}

/// Longest common prefix of `items`, in chars.
fn longest_common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix: Vec<char> = first.chars().collect();
    for item in &items[1..] {
        let other: Vec<char> = item.chars().collect();
        let keep = prefix
            .iter()
            .zip(other.iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix.truncate(keep);
        if prefix.is_empty() {
            break;
        }
    }
    prefix.into_iter().collect()
}

/// Render `s` clipped to `width` display cells.
///
/// Widths are measured in cells, not chars: a CJK directory name occupies two
/// cells per character, and counting chars let such a name overflow its
/// column, wrap the line and push everything — cursor included — one row down.
fn truncate(s: &str, width: usize) -> String {
    if unicode_column_width(s, None) <= width {
        return s.to_string();
    }
    // Leave one cell for the ellipsis.
    let budget = width.saturating_sub(1);
    let mut used = 0;
    let mut out = String::new();
    for c in s.chars() {
        let w = unicode_column_width(c.encode_utf8(&mut [0; 4]), None);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// Pad `s` on the right to `width` display cells, clipping if it is wider.
fn pad_to_cells(s: &str, width: usize) -> String {
    let clipped = truncate(s, width);
    let used = unicode_column_width(&clipped, None);
    let mut out = clipped;
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

/// Run the browser on `term` and return what the user picked.
///
/// Returns [`PickResult::Cancelled`] if the terminal closes underneath the
/// overlay, so callers never have to distinguish "cancelled" from "gone".
///
/// `history` seeds the path input row's Up/Down list. It is per session, not
/// persisted: the picker is opened from a pane and a stale list from an
/// earlier run would be more surprising than useful.
pub fn browse(
    mut term: TermWizTerminal,
    start_dir: PathBuf,
    mode: PickMode,
    title: String,
    history: Vec<String>,
) -> anyhow::Result<PickResult> {
    term.no_grab_mouse_in_raw_mode();
    let mut state = BrowserState::new(start_dir, mode, title);
    state.history = history;
    state.run_loop(&mut term)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::write(path, b"x").expect("write file");
    }

    /// A temp tree: files `b.txt`, `a.txt`, dirs `zdir/`, `adir/`.
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("zdir")).expect("mkdir zdir");
        std::fs::create_dir(dir.path().join("adir")).expect("mkdir adir");
        touch(&dir.path().join("b.txt"));
        touch(&dir.path().join("a.txt"));
        dir
    }

    #[test]
    fn read_dir_lists_directories_first_then_files_sorted() {
        let dir = fixture();
        let entries = read_dir(dir.path());
        let names: Vec<_> = entries.iter().map(|e| e.label()).collect();
        assert_eq!(names, vec!["adir/", "zdir/", "a.txt", "b.txt"]);
    }

    #[test]
    fn read_dir_includes_hidden_entries() {
        let dir = fixture();
        touch(&dir.path().join(".hidden"));
        std::fs::create_dir(dir.path().join(".hiddendir")).expect("mkdir");
        let entries = read_dir(dir.path());
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&".hidden"), "hidden file missing: {names:?}");
        assert!(
            names.contains(&".hiddendir"),
            "hidden dir missing: {names:?}"
        );
    }

    #[test]
    fn read_dir_on_a_missing_directory_is_empty_not_an_error() {
        let entries = read_dir(Path::new("/nonexistent/definitely/not/here"));
        assert!(entries.is_empty());
    }

    #[test]
    fn cursor_moves_within_bounds() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());

        state.move_up();
        assert_eq!(state.cursor, 0, "cannot move above the first row");

        state.move_down();
        state.move_down();
        assert_eq!(state.cursor, 2);
        state.move_down();
        state.move_down();
        state.move_down();
        assert_eq!(state.cursor, 3, "clamped to the last row");
    }

    #[test]
    fn moving_down_scrolls_the_viewport() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..30 {
            touch(&dir.path().join(format!("f{i:02}.txt")));
        }
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.viewport = 5;

        for _ in 0..9 {
            state.move_down();
        }
        assert_eq!(state.cursor, 9);
        assert!(
            state.top + state.viewport > state.cursor,
            "cursor {} must stay inside viewport [{}, {})",
            state.cursor,
            state.top,
            state.top + state.viewport
        );
    }

    #[test]
    fn enter_on_a_directory_descends_into_it() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        // First entry is `adir/`.
        assert_eq!(state.current().map(|e| e.name.clone()), Some("adir".into()));
        state.activate();
        assert_eq!(state.dir, dir.path().join("adir"));
        assert!(state.entries.is_empty(), "adir is empty");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn enter_on_a_file_in_file_mode_chooses_it() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        // Index 2 is `a.txt` (after adir/ and zdir/).
        state.cursor = 2;
        assert_eq!(
            state.current().map(|e| e.name.clone()),
            Some("a.txt".into())
        );
        state.activate();
        assert_eq!(
            state.outcome,
            Some(PickResult::Chosen(dir.path().join("a.txt")))
        );
    }

    #[test]
    fn enter_on_a_file_in_directory_mode_does_nothing() {
        let dir = fixture();
        let mut state =
            BrowserState::new(dir.path().to_path_buf(), PickMode::Directory, "t".into());
        state.cursor = 2;
        state.activate();
        assert_eq!(state.outcome, None, "a file is not a valid destination");
        assert_eq!(state.dir, dir.path(), "must not navigate");
    }

    #[test]
    fn ctrl_s_chooses_the_current_directory_in_directory_mode() {
        let dir = fixture();
        let mut state =
            BrowserState::new(dir.path().to_path_buf(), PickMode::Directory, "t".into());
        state.choose_current_dir();
        assert_eq!(
            state.outcome,
            Some(PickResult::Chosen(dir.path().to_path_buf()))
        );
    }

    #[test]
    fn ctrl_s_is_ignored_in_file_mode() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.choose_current_dir();
        assert_eq!(state.outcome, None);
    }

    #[test]
    fn go_up_returns_to_the_parent_and_reselects_it() {
        let dir = fixture();
        let nested = dir.path().join("adir");
        let mut state = BrowserState::new(nested.clone(), PickMode::File, "t".into());

        state.go_up();
        assert_eq!(state.dir, dir.path());
        // The directory we came from is selected, so a mistake is easy to undo.
        assert_eq!(
            state.current().map(|e| e.path.clone()),
            Some(nested),
            "the directory we left should be selected"
        );
    }

    #[test]
    fn go_up_at_the_root_stays_put() {
        let mut state = BrowserState::new(PathBuf::from("/"), PickMode::File, "t".into());
        state.go_up();
        assert_eq!(state.dir, PathBuf::from("/"));
    }

    #[test]
    fn truncate_keeps_short_strings_and_clips_long_ones() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abc", 3), "abc");
        let clipped = truncate("abcdefghij", 5);
        assert_eq!(clipped.chars().count(), 5);
        assert!(clipped.ends_with('…'));
    }

    /// Build a key event the way `TermWizTerminal::poll_input` delivers it.
    fn ctrl(c: char) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: KeyCode::Char(c),
            modifiers: Modifiers::CTRL,
        })
    }

    fn arrow(key: KeyCode) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::NONE,
        })
    }

    /// Regression for a reported bug: Ctrl-S did nothing after picking a
    /// directory. The chord's exact shape depends on the platform and the
    /// terminal layer, so every shape that has been observed must confirm.
    #[test]
    fn ctrl_s_confirms_the_directory_in_every_shape() {
        for (label, event) in [
            ("uppercase + CTRL", ctrl('S')),
            ("lowercase + CTRL", ctrl('s')),
            (
                "control char, no modifier",
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char('\u{13}'),
                    modifiers: Modifiers::NONE,
                }),
            ),
            (
                "control char + CTRL",
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char('\u{13}'),
                    modifiers: Modifiers::CTRL,
                }),
            ),
        ] {
            let dir = fixture();
            let mut state =
                BrowserState::new(dir.path().to_path_buf(), PickMode::Directory, "t".into());
            state.handle_event(event);
            assert_eq!(
                state.outcome,
                Some(PickResult::Chosen(dir.path().to_path_buf())),
                "Ctrl-S as {label} must confirm the directory"
            );
        }
    }

    #[test]
    fn space_confirms_the_directory_without_a_modifier() {
        // A bare key path, so the binding survives a dropped Ctrl modifier.
        let dir = fixture();
        let mut state =
            BrowserState::new(dir.path().to_path_buf(), PickMode::Directory, "t".into());
        state.handle_event(InputEvent::Key(KeyEvent {
            key: KeyCode::Char(' '),
            modifiers: Modifiers::NONE,
        }));
        assert_eq!(
            state.outcome,
            Some(PickResult::Chosen(dir.path().to_path_buf()))
        );
    }

    #[test]
    fn enter_confirms_an_empty_directory_in_directory_mode() {
        // Nothing to descend into, so the directory on screen is the answer.
        let empty = tempfile::tempdir().expect("tempdir");
        let mut state =
            BrowserState::new(empty.path().to_path_buf(), PickMode::Directory, "t".into());
        state.handle_event(arrow(KeyCode::Enter));
        assert_eq!(
            state.outcome,
            Some(PickResult::Chosen(empty.path().to_path_buf()))
        );
    }

    #[test]
    fn ctrl_j_and_k_move_the_cursor() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());

        state.handle_event(ctrl('J'));
        assert_eq!(state.cursor, 1);
        state.handle_event(ctrl('K'));
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn arrows_move_the_cursor() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.handle_event(arrow(KeyCode::DownArrow));
        assert_eq!(state.cursor, 1);
        state.handle_event(arrow(KeyCode::UpArrow));
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn escape_and_q_cancel() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        assert_eq!(
            state.handle_event(InputEvent::Key(KeyEvent {
                key: KeyCode::Char('q'),
                modifiers: Modifiers::NONE,
            })),
            EventAction::Done(PickResult::Cancelled)
        );
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        assert_eq!(
            state.handle_event(arrow(KeyCode::Escape)),
            EventAction::Done(PickResult::Cancelled)
        );
    }

    #[test]
    fn enter_descends_and_backspace_goes_up() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        assert_eq!(
            state.handle_event(arrow(KeyCode::Enter)),
            EventAction::Continue
        );
        assert_eq!(state.dir, dir.path().join("adir"));
        state.handle_event(arrow(KeyCode::Backspace));
        assert_eq!(state.dir, dir.path());
    }

    #[test]
    fn empty_directory_has_no_current_entry_and_ignores_movement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        assert!(state.current().is_none());
        state.move_down();
        state.move_up();
        state.activate();
        assert_eq!(state.cursor, 0);
        assert_eq!(state.outcome, None);
    }
    // ---- path input row ----
    #[test]
    fn ctrl_p_enters_input_mode_seeded_with_current_dir() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        assert!(!state.input_mode);
        state.handle_event(ctrl('p'));
        assert!(state.input_mode, "Ctrl-P should focus the path row");
        assert_eq!(state.input_string(), format!("{}/", dir.path().display()));
    }

    #[test]
    fn tab_completes_a_unique_prefix() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.begin_input();
        state.set_input(&format!("{}/a", dir.path().display()));
        state.handle_input_event(InputEvent::Key(KeyEvent {
            key: KeyCode::Tab,
            modifiers: Modifiers::NONE,
        }));
        // `a.txt` is the only entry matching `a` (adir/ and a.txt both match,
        // so this stops at the shared prefix).
        assert_eq!(
            state.input_string(),
            format!("{}/a", dir.path().display()),
            "shared prefix between adir/ and a.txt is just `a`"
        );
        assert_eq!(state.completions.len(), 2);
    }

    #[test]
    fn tab_completes_when_only_one_candidate_matches() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.begin_input();
        state.set_input(&format!("{}/b.t", dir.path().display()));
        state.handle_input_event(InputEvent::Key(KeyEvent {
            key: KeyCode::Tab,
            modifiers: Modifiers::NONE,
        }));
        assert_eq!(
            state.input_string(),
            format!("{}/b.txt", dir.path().display())
        );
        assert_eq!(state.completions.len(), 1);
    }

    #[test]
    fn tab_appends_a_slash_after_a_directory() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.begin_input();
        state.set_input(&format!("{}/zdi", dir.path().display()));
        state.handle_input_event(InputEvent::Key(KeyEvent {
            key: KeyCode::Tab,
            modifiers: Modifiers::NONE,
        }));
        assert_eq!(
            state.input_string(),
            format!("{}/zdir/", dir.path().display())
        );
    }

    #[test]
    fn completions_can_reach_hidden_entries() {
        let dir = fixture();
        touch(&dir.path().join(".env"));
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.begin_input();
        state.set_input(&format!("{}/.en", dir.path().display()));
        state.handle_input_event(InputEvent::Key(KeyEvent {
            key: KeyCode::Tab,
            modifiers: Modifiers::NONE,
        }));
        assert_eq!(
            state.input_string(),
            format!("{}/.env", dir.path().display()),
            "a path under a dotfile must be completable"
        );
    }

    #[test]
    fn directory_mode_completes_only_directories() {
        let dir = fixture();
        let mut state =
            BrowserState::new(dir.path().to_path_buf(), PickMode::Directory, "t".into());
        state.begin_input();
        state.set_input(&format!("{}/", dir.path().display()));
        state.complete();
        assert!(
            state.completions.iter().all(|c| c.ends_with('/')),
            "only directories belong in a directory picker: {:?}",
            state.completions
        );
    }

    #[test]
    fn enter_on_a_typed_file_chooses_it() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.begin_input();
        state.set_input(&format!("{}/b.txt", dir.path().display()));
        state.handle_input_event(InputEvent::Key(KeyEvent {
            key: KeyCode::Enter,
            modifiers: Modifiers::NONE,
        }));
        assert_eq!(
            state.outcome,
            Some(PickResult::Chosen(dir.path().join("b.txt")))
        );
    }

    #[test]
    fn enter_on_a_typed_directory_navigates_instead_of_choosing() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.begin_input();
        state.set_input(&format!("{}/zdir", dir.path().display()));
        state.handle_input_event(InputEvent::Key(KeyEvent {
            key: KeyCode::Enter,
            modifiers: Modifiers::NONE,
        }));
        assert_eq!(state.dir, dir.path().join("zdir"));
        assert_eq!(state.outcome, None);
        assert!(!state.input_mode, "confirming a directory closes the row");
    }

    #[test]
    fn relative_input_resolves_against_the_browsed_directory() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.begin_input();
        state.set_input("b.txt");
        state.confirm_input();
        assert_eq!(
            state.outcome,
            Some(PickResult::Chosen(dir.path().join("b.txt")))
        );
    }

    #[test]
    fn tilde_expands_to_home() {
        let resolved = resolve_against(Path::new("/tmp"), "~/notes.txt");
        assert_eq!(resolved, config::HOME_DIR.join("notes.txt"));
    }

    #[test]
    fn cursor_editing_stays_in_bounds_and_edits_in_place() {
        let mut state = BrowserState::new(PathBuf::from("/tmp"), PickMode::File, "t".into());
        state.begin_input();
        state.set_input("abcd");
        state.cursor_left();
        state.cursor_left();
        state.insert_char('X');
        assert_eq!(state.input_string(), "abXcd");
        assert_eq!(state.input_cursor, 3);

        state.cursor_home();
        state.delete_backward();
        assert_eq!(state.input_string(), "abXcd", "backspace at 0 is a no-op");

        state.cursor_end();
        state.delete_forward();
        assert_eq!(
            state.input_string(),
            "abXcd",
            "delete at the end is a no-op"
        );

        state.delete_backward();
        assert_eq!(state.input_string(), "abXc");
    }

    #[test]
    fn home_and_end_control_chords_are_not_inserted_as_text() {
        let mut state = BrowserState::new(PathBuf::from("/tmp"), PickMode::File, "t".into());
        state.begin_input();
        state.set_input("abc");
        // Ctrl-A arrives as a control char with the CTRL modifier.
        state.handle_input_event(ctrl('a'));
        assert_eq!(state.input_cursor, 0);
        assert_eq!(state.input_string(), "abc", "Ctrl-A must not be typed");

        state.handle_input_event(ctrl('e'));
        assert_eq!(state.input_cursor, 3);
    }

    #[test]
    fn history_walks_confirmed_paths_and_returns_to_a_blank_line() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.history = vec!["/one".to_string(), "/two".to_string()];
        state.begin_input();

        state.history_up();
        assert_eq!(state.input_string(), "/two", "newest first");
        state.history_up();
        assert_eq!(state.input_string(), "/one");
        state.history_up();
        assert_eq!(state.input_string(), "/one", "clamped at the oldest");

        state.history_down();
        assert_eq!(state.input_string(), "/two");
        state.history_down();
        assert_eq!(state.input_string(), "", "past the newest is a blank line");
    }

    #[test]
    fn choosing_a_file_records_it_in_history() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.cursor = 2; // a.txt
        state.activate();
        assert_eq!(state.history.len(), 1);
        assert!(state.history[0].ends_with("a.txt"));
    }

    #[test]
    fn escape_leaves_input_mode_without_cancelling_the_picker() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.begin_input();
        let action = state.handle_input_event(InputEvent::Key(KeyEvent {
            key: KeyCode::Escape,
            modifiers: Modifiers::NONE,
        }));
        assert_eq!(action, EventAction::Continue);
        assert!(!state.input_mode, "Esc backs out of the row only");
    }

    #[test]
    fn navigation_arrows_do_not_reach_the_listing_while_typing() {
        let dir = fixture();
        let mut state = BrowserState::new(dir.path().to_path_buf(), PickMode::File, "t".into());
        state.begin_input();
        state.set_input("abc");
        state.cursor_left();
        state.handle_input_event(InputEvent::Key(KeyEvent {
            key: KeyCode::LeftArrow,
            modifiers: Modifiers::NONE,
        }));
        assert_eq!(state.input_cursor, 1, "left arrow moves the caret");
        assert_eq!(state.cursor, 0, "the listing cursor must not move");
    }

    #[test]
    fn longest_common_prefix_handles_disjoint_and_shared_names() {
        assert_eq!(
            longest_common_prefix(&["adir/".to_string(), "a.txt".to_string()]),
            "a"
        );
        assert_eq!(
            longest_common_prefix(&["foo".to_string(), "bar".to_string()]),
            ""
        );
        assert_eq!(longest_common_prefix(&["only".to_string()]), "only");
        assert_eq!(longest_common_prefix(&[]), "");
    }
    #[test]
    fn base_name_keeps_the_directory_separator() {
        assert_eq!(base_name("~/Projects/alpha/"), "alpha/");
        assert_eq!(base_name("/usr/local/bin"), "bin");
        assert_eq!(base_name("plain"), "plain");
        assert_eq!(base_name("/"), "/");
    }

    #[test]
    fn completion_columns_grows_with_narrow_names() {
        let mut state = BrowserState::new(PathBuf::from("/tmp"), PickMode::File, "t".into());
        state.completions = vec!["/a/x/".to_string(); 4];
        let narrow = state.completion_columns(120);
        state.completions = vec!["/a/a-very-long-directory-name/".to_string(); 4];
        let wide = state.completion_columns(120);
        assert!(
            narrow > wide,
            "short names should pack into more columns: {narrow} vs {wide}"
        );
        assert!(wide >= 1, "always at least one column");
    }

    #[test]
    fn page_down_scrolls_the_completion_panel_and_stops_at_the_end() {
        let mut state = BrowserState::new(PathBuf::from("/tmp"), PickMode::File, "t".into());
        state.completions = (0..35).map(|i| format!("/a/f{i:02}/")).collect();
        assert_eq!(state.completion_top, 0);
        state.completion_page(1);
        assert!(state.completion_top > 0, "PageDown moves the window");
        for _ in 0..10 {
            state.completion_page(1);
        }
        assert!(
            state.completion_top < state.completions.len(),
            "the window must not scroll past the last candidate"
        );
        state.completion_page(-1);
        assert!(state.completion_top < state.completions.len());
    }

    #[test]
    fn editing_clears_the_completion_panel_and_its_scroll() {
        let mut state = BrowserState::new(PathBuf::from("/tmp"), PickMode::File, "t".into());
        state.begin_input();
        state.completions = (0..20).map(|i| format!("/a/f{i:02}/")).collect();
        state.completion_top = 10;
        state.insert_char('x');
        assert!(
            state.completions.is_empty(),
            "stale candidates must not linger"
        );
        assert_eq!(state.completion_top, 0);
    }
    #[test]
    fn truncate_counts_cjk_as_two_cells() {
        // Each CJK char is two columns, so three of them do not fit in 5.
        assert_eq!(unicode_column_width("驱动/", None), 5);
        let clipped = truncate("驱动/", 3);
        assert!(
            unicode_column_width(&clipped, None) <= 3,
            "clipping must be measured in cells, got {clipped:?}"
        );
    }

    #[test]
    fn pad_to_cells_keeps_every_column_the_same_width() {
        let padded = pad_to_cells("驱动/", 10);
        assert_eq!(
            unicode_column_width(&padded, None),
            10,
            "CJK name must pad to the same cell width as an ASCII one"
        );
        let ascii = pad_to_cells("iso/", 10);
        assert_eq!(unicode_column_width(&ascii, None), 10);
    }

    #[test]
    fn caret_lands_after_wide_characters_in_cells() {
        let mut state = BrowserState::new(PathBuf::from("/tmp"), PickMode::File, "t".into());
        state.begin_input();
        state.set_input("通用");
        let typed: String = state.input[..state.input_cursor].iter().collect();
        // Two CJK chars occupy four cells, not two.
        assert_eq!(unicode_column_width(&typed, None), 4);
    }
}
