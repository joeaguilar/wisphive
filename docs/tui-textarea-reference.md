# tui-textarea Reference

Integration reference for [tui-textarea](https://github.com/rhysd/tui-textarea) v0.7.0 in wisphive TUI modals.

## Dependency

```toml
# Cargo.toml — requires ratatui 0.29.0 (already in workspace)
tui-textarea = { version = "0.7.0", default-features = false, features = ["crossterm"] }
```

## Core Patterns

### Creating a TextArea

```rust
use tui_textarea::TextArea;

// Empty
let mut textarea = TextArea::default();

// Pre-filled
let mut textarea = TextArea::new(vec!["initial text".to_string()]);

// From iterator
let mut textarea = TextArea::from(["line 1", "line 2"]);
```

### Single-Line Input (our modal use case)

Block Enter to prevent newlines. Use `input_without_shortcuts` or filter Enter in a match:

```rust
match crossterm::event::read()?.into() {
    Input { key: Key::Enter, .. } => { /* submit */ }
    Input { key: Key::Esc, .. } => { /* cancel */ }
    // Block Ctrl+M (also Enter) from inserting newline
    Input { key: Key::Char('m'), ctrl: true, .. } => {}
    input => { textarea.input(input); }
}
```

### Rendering

TextArea implements `Widget` — render directly:

```rust
frame.render_widget(&textarea, rect);
```

### Getting Text

```rust
// Borrow lines (never empty — empty text = one empty string)
let text = textarea.lines()[0].clone();
let all_text = textarea.lines().join("\n");

// Consume
let lines: Vec<String> = textarea.into_lines();
```

### Checking Emptiness

```rust
textarea.is_empty()  // true when content is empty
```

## Styling

```rust
use ratatui::style::{Color, Style, Modifier};
use ratatui::widgets::{Block, Borders};

// Base text style
textarea.set_style(Style::default().fg(Color::Yellow));

// Border block
textarea.set_block(
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightBlue))
        .title("Title"),
);

// Cursor style (default: reversed)
textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));

// Cursor line highlight (default: underline) — disable with empty style
textarea.set_cursor_line_style(Style::default());

// Placeholder (shown when empty)
textarea.set_placeholder_text("Type here...");
textarea.set_placeholder_style(Style::default().fg(Color::DarkGray));

// Password masking
textarea.set_mask_char('*');
textarea.clear_mask_char();

// Line numbers (disabled by default)
textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
textarea.remove_line_number();

// Selection highlight (default: light blue bg)
textarea.set_selection_style(Style::default().bg(Color::Blue));

// Text alignment
textarea.set_alignment(Alignment::Left); // Center/Right disable line numbers
```

## Default Key Bindings

Called via `textarea.input(event)`:

| Keys | Action |
|------|--------|
| Ctrl+H, Backspace | Delete char before cursor |
| Ctrl+D, Delete | Delete char after cursor |
| Ctrl+M, Enter | Insert newline |
| Ctrl+K | Delete to end of line |
| Ctrl+J | Delete to start of line |
| Ctrl+W, Alt+Backspace | Delete word before cursor |
| Alt+D | Delete word after cursor |
| Ctrl+U | Undo |
| Ctrl+R | Redo |
| Ctrl+C | Copy selection |
| Ctrl+X | Cut selection |
| Ctrl+Y | Paste yanked text |
| Ctrl+F, Right | Move forward |
| Ctrl+B, Left | Move backward |
| Ctrl+P, Up | Move up |
| Ctrl+N, Down | Move down |
| Alt+F, Ctrl+Right | Word forward |
| Alt+B, Ctrl+Left | Word backward |
| Ctrl+A, Home | Line start |
| Ctrl+E, End | Line end |
| Alt+< | File top |
| Alt+> | File bottom |
| Ctrl+V, PageDown | Page down |
| Alt+V, PageUp | Page up |

**Important conflicts for wisphive modals:**
- Ctrl+C = copy (not quit) — we handle Ctrl+C quit before modal input
- Ctrl+U = undo (not our usual undo) — fine for text editing
- Enter = newline — must be intercepted for single-line submit

### Bypassing Shortcuts

`textarea.input_without_shortcuts(input)` — only handles basic char insert, Tab, Enter, Backspace, Delete. Use this when you want full control over key bindings.

## Programmatic Editing

```rust
// Insert
textarea.insert_char('x');
textarea.insert_str("hello world");  // handles \n
textarea.insert_newline();
textarea.insert_tab();

// Delete
textarea.delete_char();         // backspace
textarea.delete_next_char();    // delete forward
textarea.delete_line_by_head(); // to line start
textarea.delete_line_by_end();  // to line end
textarea.delete_word();         // word before
textarea.delete_next_word();    // word after
textarea.delete_str(5);         // N chars forward

// Cursor
textarea.move_cursor(CursorMove::Forward);
textarea.move_cursor(CursorMove::Jump(row, col));
textarea.cursor(); // returns (row, col)

// Selection
textarea.start_selection();
textarea.cancel_selection();
textarea.select_all();
textarea.is_selecting();
textarea.selection_range(); // Option<((row,col),(row,col))>
textarea.copy();
textarea.cut();
textarea.paste();

// Yank buffer
textarea.yank_text();
textarea.set_yank_text("text");

// History
textarea.undo();
textarea.redo();
textarea.set_max_histories(100); // default: 50, 0 = disable

// Scrolling
textarea.scroll(Scrolling::PageDown);
```

## Search (requires `search` feature)

```rust
textarea.set_search_pattern("regex").unwrap();
textarea.search_forward(false);  // false = don't match at cursor
textarea.search_back(false);
textarea.set_search_pattern("").unwrap(); // stop searching
textarea.set_search_style(Style::default().bg(Color::Yellow));
```

## Integration Notes for Wisphive

### Modal text inputs to replace

1. **TextInputModal** (deny-with-message, approve-with-context) — single-line, replace `buffer: String` with `TextArea`
2. **EditInputModal** (edit tool input) — multi-line, replace `buffer: String` with `TextArea`
3. **SpawnModal** (project + prompt fields) — two single-line TextAreas replacing `project_buf`/`prompt_buf`

### Key conflicts to handle

- **Esc**: TextArea doesn't consume Esc — we use it to cancel modals (safe)
- **Enter**: Must intercept before `textarea.input()` for single-line submit
- **Ctrl+C**: TextArea uses for copy — our quit handler runs before modal input (safe)
- **Ctrl+N/P**: TextArea uses for up/down movement — fine in text context

### Rendering approach

Replace manual `Span`-based line building with `frame.render_widget(&textarea, input_rect)` inside the modal area. Use ratatui `Layout` to split the modal area into body text + input widget regions.
