# ur_markdown

Terminal markdown rendering crate for the Ur coordination framework. Converts markdown text into `Vec<ratatui::text::Line>` with styled spans using pulldown-cmark for parsing.

## Purpose

Used by urui screens (e.g., `TicketDetailScreen`, `TicketBodyScreen`) to render ticket body text as styled terminal output.

## Public API

- `MarkdownColors` — Plain struct with four `ratatui::style::Color` fields: `text`, `heading`, `code`, `dim`. Passed by the caller instead of a full `Theme` to avoid a circular dependency with urui.
- `render_markdown(text, width, colors) -> Vec<Line<'static>>` — Top-level rendering function. Parses markdown via pulldown-cmark and returns styled lines word-wrapped to the given column width.

## Design Notes

- **No urui dependency** — Takes `MarkdownColors`, not `Theme`, to prevent a dependency cycle.
- **Static lifetimes** — All returned `Line<'static>` values own their string data (`.to_owned()` / `into_string()`).
- **Unhandled elements** — Links, images, tables, and raw HTML are silently degraded to plain text or discarded rather than causing errors.
- **Wrapping** — Plain paragraph text is word-wrapped to `width` columns. Headings and code blocks are not wrapped.

## Dependencies

- `pulldown-cmark` — Markdown parsing (CommonMark with strikethrough and task-list extensions).
- `ratatui` — `Line`, `Span`, `Style`, `Color`, `Modifier` types.
