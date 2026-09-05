use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame,
};

use crate::state::TuiState;
use crate::theme::HadesTheme;
use hades_core::{AppState, HadesApp};

/// Main draw entry point dispatching view rendering with clean full-height conversation layout.
pub fn render(frame: &mut Frame, app: &HadesApp, state: &mut TuiState) {
    let size = frame.area();
    if size.width < 10 || size.height < 5 {
        // Guard against degenerate terminal sizes during extreme resize
        return;
    }

    // Dynamic input height based on text wrapping and multiline inputs
    let max_input_height = size.height.saturating_sub(4).clamp(1, 8);
    let input_height = calculate_input_height(&state.prompt_input, size.width, max_input_height);

    let [chat_area, top_border_area, input_area, bottom_border_area, status_area] =
        compute_layout_chunks(size, input_height);

    render_conversation(frame, app, state, chat_area);
    render_input_top_border(frame, top_border_area);
    render_input_area(frame, app, state, input_area);
    render_input_bottom_border(frame, bottom_border_area);
    render_status_bar(frame, app, state, status_area);

    // In-terminal Attention Banner Pop-In when Input Required
    if app.state() == AppState::ToolApproval {
        let alert_text = " 🚨 ATTENTION: USER INPUT REQUIRED (PRESS 1-4 TO AUTHORIZE) ";
        let alert_len = alert_text.chars().count() as u16;
        let alert_width = alert_len.min(size.width.saturating_sub(4));
        let alert_area = Rect {
            x: size.width.saturating_sub(alert_width + 2),
            y: 0,
            width: alert_width,
            height: 1,
        };
        frame.render_widget(Clear, alert_area);
        let alert_line = Line::from(vec![Span::styled(
            alert_text,
            Style::default()
                .fg(Color::White)
                .bg(HadesTheme::RATATUI_FIRE)
                .add_modifier(Modifier::BOLD),
        )]);
        frame.render_widget(Paragraph::new(alert_line), alert_area);
    }

    // Floating Ephemeral Toast Notification (e.g. "✓ Copied assistant response to clipboard")
    if let Some(toast) = state.toast_text() {
        let toast_len = (toast.chars().count() + 6) as u16;
        let toast_width = toast_len.min(size.width.saturating_sub(4));
        let toast_area = Rect {
            x: size.width.saturating_sub(toast_width + 2),
            y: chat_area.y + chat_area.height.saturating_sub(2),
            width: toast_width,
            height: 1,
        };
        frame.render_widget(Clear, toast_area);
        let toast_line = Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                toast,
                Style::default()
                    .fg(HadesTheme::RATATUI_GOLD)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default()),
        ]);
        let para = Paragraph::new(toast_line).style(Style::default().bg(Color::Rgb(35, 35, 35)));
        frame.render_widget(para, toast_area);
    }

    // Modal Overlays (Minimal, clean floating dialogs)
    match app.state() {
        AppState::CommandPalette => render_command_palette(frame, app, state, size),
        AppState::SessionSelect => render_session_select(frame, app, state, size),
        AppState::SessionRename => render_session_rename(frame, state, size),
        AppState::SessionDeleteConfirm => render_session_delete_confirm(frame, state, size),
        AppState::ProviderSelect => render_provider_select(frame, state, size),
        AppState::ModelSelect => render_model_select(frame, state, size),
        AppState::ModelInfo => render_model_info(frame, state, size),
        AppState::CredentialInput => render_credential_input(frame, state, size),
        AppState::Verifying => render_verifying(frame, state, size),
        AppState::VerificationFailed => render_verification_failed(frame, state, size),
        AppState::ToolApproval => render_tool_approval(frame, app, state, size),
        AppState::CopySelect => render_copy_select(frame, state, size),
        AppState::McpSetup => render_mcp_setup(frame, state, size),
        _ => {}
    }
}

/// Helper formatting and word-wrapping turn texts with tree-branch indentations.
pub fn wrap_turn_text(
    text: &str,
    width: usize,
    first_prefix: &str,
    cont_prefix: &str,
    style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let prefix_width = 5; // "  └─ " or "     "
    let content_width = width.saturating_sub(prefix_width).max(10);

    for raw_line in text.lines() {
        if raw_line.is_empty() {
            lines.push(Line::from(""));
            continue;
        }

        let words = raw_line.split(' ');
        let mut current_line = String::new();
        let mut is_first = lines.is_empty();

        for word in words {
            if word.is_empty() {
                continue;
            }
            if current_line.is_empty() {
                current_line.push_str(word);
            } else if current_line.len() + 1 + word.len() <= content_width {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                let prefix = if is_first {
                    first_prefix.to_string()
                } else {
                    cont_prefix.to_string()
                };
                lines.push(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                    Span::styled(current_line, style),
                ]));
                is_first = false;
                current_line = word.to_string();
            }
        }

        if !current_line.is_empty() {
            let prefix = if is_first {
                first_prefix.to_string()
            } else {
                cont_prefix.to_string()
            };
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                Span::styled(current_line, style),
            ]));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                first_prefix.to_string(),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(text.to_string(), style),
        ]));
    }

    lines
}

/// Renders the strictly bounded conversation stream with application-owned scrollbar and auto-follow.
fn render_conversation(frame: &mut Frame, app: &HadesApp, state: &mut TuiState, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    // Split horizontally into conversation text area and 1-column scrollbar track
    let conv_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let content_area = conv_chunks[0];
    let scrollbar_area = conv_chunks[1];

    let mut lines: Vec<Line<'static>> = Vec::new();
    let width = content_area.width as usize;

    // 1. Welcome / Initial Branding Guide if conversation is empty
    if state.turns.is_empty() && state.active_output.is_none() && state.error_message.is_none() {
        lines.push(Line::from(""));
        if width >= 60 {
            lines.extend(HadesTheme::banner().lines);
        } else {
            lines.extend(HadesTheme::compact_banner().lines);
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "  Welcome to Hades.",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(vec![
            Span::styled("  Active model: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.active_model_display(),
                if app.active_model_display() == "Not configured" {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                        .fg(HadesTheme::RATATUI_ORANGE)
                        .add_modifier(Modifier::BOLD)
                },
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "  Type a prompt and press ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to chat, or ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "/",
                Style::default()
                    .fg(HadesTheme::RATATUI_ORANGE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " for commands (/help, /model, /tools, /sessions, /new).",
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        lines.push(Line::from(""));
    }

    // 2. Global Error Banner, if any
    if let Some(ref err) = state.error_message {
        lines.push(Line::from(vec![
            Span::styled(
                "  Error: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(err.clone(), Style::default().fg(Color::Red)),
        ]));
        lines.push(Line::from(""));
    }

    // 3. Active Command Output, if any
    if let Some(ref output) = state.active_output {
        let out_str = output.to_string();
        for l in out_str.lines() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(l.to_string(), Style::default().fg(Color::White)),
            ]));
        }
        lines.push(Line::from(""));
    }

    // 4. Chronological Conversation Turns with Dynamic Width-Aware Wrapping
    for turn in &state.turns {
        // User turn header
        lines.push(Line::from(vec![Span::styled(
            "  You",
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )]));

        // User turn content (indented with tree branch)
        let user_lines = wrap_turn_text(
            &turn.user_prompt,
            width,
            "  └─ ",
            "     ",
            Style::default().fg(Color::White),
        );
        lines.extend(user_lines);
        lines.push(Line::from(""));

        // Hades turn response / activity / error
        lines.push(Line::from(vec![Span::styled(
            "  Hades",
            Style::default()
                .fg(HadesTheme::RATATUI_GOLD)
                .add_modifier(Modifier::BOLD),
        )]));

        if let Some(ref activity) = turn.activity_text {
            lines.push(Line::from(vec![
                Span::styled("  └─ ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} ", state.spinner_char()),
                    Style::default().fg(HadesTheme::RATATUI_GOLD),
                ),
                Span::styled(
                    activity.clone(),
                    Style::default().fg(HadesTheme::RATATUI_GOLD),
                ),
            ]));
        } else if let Some(ref response) = turn.assistant_response {
            let resp_lines = wrap_turn_text(
                response,
                width,
                "  └─ ",
                "     ",
                Style::default().fg(Color::White),
            );
            lines.extend(resp_lines);
        } else if let Some(ref err) = turn.error_text {
            lines.push(Line::from(vec![
                Span::styled(
                    "  └─ Error: ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(err.clone(), Style::default().fg(Color::Red)),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Synchronize geometry and scroll offset safely
    let total_lines = lines.len();
    let viewport_height = content_area.height as usize;
    state.update_geometry(total_lines, viewport_height);

    let max_scroll = state.max_scroll_offset();
    let scroll_y = state.scroll_offset;

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y as u16, 0));

    frame.render_widget(paragraph, content_area);

    // 5. Render Application-Owned Scrollbar Widget
    let scrollbar = Scrollbar::default()
        .orientation(ScrollbarOrientation::VerticalRight)
        .thumb_symbol("█")
        .track_symbol(Some("░"))
        .begin_symbol(Some("▲"))
        .end_symbol(Some("▼"))
        .thumb_style(Style::default().fg(HadesTheme::RATATUI_ORANGE))
        .track_style(Style::default().fg(Color::DarkGray));

    let mut scrollbar_state = ScrollbarState::new(max_scroll).position(scroll_y);
    frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);

    // 6. Subtle New-Content Indicator when scrolled away from the bottom
    if state.has_new_content_below {
        let indicator_line = Line::from(vec![
            Span::styled(
                "  ↓ New content below (press ",
                Style::default().fg(HadesTheme::RATATUI_GOLD),
            ),
            Span::styled(
                "End",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to follow)  ",
                Style::default().fg(HadesTheme::RATATUI_GOLD),
            ),
        ]);
        let indicator_width = 38u16.min(content_area.width);
        let indicator_area = Rect {
            x: content_area.x + content_area.width.saturating_sub(indicator_width + 1),
            y: content_area.y + content_area.height.saturating_sub(1),
            width: indicator_width,
            height: 1,
        };
        frame.render_widget(Clear, indicator_area);
        frame.render_widget(
            Paragraph::new(indicator_line).style(Style::default().bg(Color::Rgb(40, 40, 40))),
            indicator_area,
        );
    }
}

/// Helper estimating wrapped line count given line texts and terminal width.
pub fn estimate_wrapped_line_count(lines: &[Line], width: u16) -> usize {
    if width == 0 {
        return lines.len();
    }
    let w = width as usize;
    let mut total = 0;
    for line in lines {
        let line_len: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        if line_len == 0 {
            total += 1;
        } else {
            total += line_len.div_ceil(w);
        }
    }
    total
}

/// Computes the authoritative vertical layout chunks for the full TUI screen.
///
/// Returns 5 non-overlapping rectangular regions in order:
/// 0. Chat conversation viewport (`Constraint::Min(1)`)
/// 1. Input top border (`Constraint::Length(1)`)
/// 2. Input prompt/content (`Constraint::Length(input_height)`)
/// 3. Input bottom border (`Constraint::Length(1)`)
/// 4. Status bar / footer (`Constraint::Length(1)`)
pub fn compute_layout_chunks(size: Rect, input_height: u16) -> [Rect; 5] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(input_height.max(1)),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(size);

    [chunks[0], chunks[1], chunks[2], chunks[3], chunks[4]]
}

/// Calculates the dynamic height of the input prompt area based on text length, newlines, and terminal width.
pub fn calculate_input_height(prompt: &str, width: u16, max_height: u16) -> u16 {
    if width == 0 || max_height == 0 {
        return 1;
    }
    let avail_width = (width as usize).saturating_sub(4).max(10);
    let mut total_lines: u16 = 0;
    for raw_line in prompt.split('\n') {
        let char_count = raw_line.chars().count();
        let wrapped = if char_count == 0 {
            1
        } else {
            (char_count.div_ceil(avail_width) as u16).max(1)
        };
        total_lines = total_lines.saturating_add(wrapped);
    }
    total_lines.clamp(1, max_height)
}

/// Renders the full-width top horizontal border of the input box.
pub fn render_input_top_border(frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let separator = "─".repeat(area.width as usize);
    let sep_para = Paragraph::new(Line::from(vec![Span::styled(
        separator,
        Style::default().fg(Color::DarkGray),
    )]));
    frame.render_widget(sep_para, area);
}

/// Renders the prompt input area between the top and bottom borders.
pub fn render_input_area(frame: &mut Frame, app: &HadesApp, state: &TuiState, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let is_generating = app.state() == AppState::AiThinking || app.state() == AppState::AiStreaming;

    let prompt_color = if is_generating {
        Color::DarkGray
    } else {
        HadesTheme::RATATUI_ORANGE
    };

    let mut lines = Vec::new();

    if is_generating {
        lines.push(Line::from(vec![
            Span::styled(
                " › ",
                Style::default()
                    .fg(prompt_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(&state.prompt_input, Style::default().fg(Color::White)),
            Span::styled("▌", Style::default().fg(Color::DarkGray)),
            Span::styled(
                " (generating response...)",
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    } else if state.prompt_input.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                " › ",
                Style::default()
                    .fg(prompt_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", Style::default().fg(HadesTheme::RATATUI_ORANGE)),
        ]));
    } else {
        let prompt_lines: Vec<&str> = state.prompt_input.split('\n').collect();
        let count = prompt_lines.len();
        for (i, p_line) in prompt_lines.into_iter().enumerate() {
            if i == 0 {
                let mut spans = vec![
                    Span::styled(
                        " › ",
                        Style::default()
                            .fg(prompt_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(p_line, Style::default().fg(Color::White)),
                ];
                if i == count - 1 {
                    spans.push(Span::styled(
                        "▌",
                        Style::default().fg(HadesTheme::RATATUI_ORANGE),
                    ));
                }
                lines.push(Line::from(spans));
            } else {
                let mut spans = vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(p_line, Style::default().fg(Color::White)),
                ];
                if i == count - 1 {
                    spans.push(Span::styled(
                        "▌",
                        Style::default().fg(HadesTheme::RATATUI_ORANGE),
                    ));
                }
                lines.push(Line::from(spans));
            }
        }
    }

    let prompt_para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(prompt_para, area);
}

/// Renders the full-width bottom horizontal border of the input box.
pub fn render_input_bottom_border(frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let separator = "─".repeat(area.width as usize);
    let sep_para = Paragraph::new(Line::from(vec![Span::styled(
        separator,
        Style::default().fg(Color::DarkGray),
    )]));
    frame.render_widget(sep_para, area);
}

/// Renders the compact status line pinned to the bottom row of the terminal.
fn render_status_bar(frame: &mut Frame, app: &HadesApp, state: &TuiState, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let ws_name = app.workspace().name();
    let model_display = app.active_model_display();
    let mode_display = &app.config().general.default_mode;

    let status_line = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled("📁 ", Style::default().fg(HadesTheme::RATATUI_ORANGE)),
        Span::styled(
            ws_name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            if model_display == "Not configured" {
                "No Model"
            } else {
                &model_display
            },
            if model_display == "Not configured" {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
                    .fg(HadesTheme::RATATUI_ORANGE)
                    .add_modifier(Modifier::BOLD)
            },
        ),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            mode_display,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        if app.state() == AppState::ToolApproval {
            Span::styled(
                "[🔔 INPUT REQUIRED]",
                Style::default()
                    .fg(Color::Black)
                    .bg(HadesTheme::RATATUI_GOLD)
                    .add_modifier(Modifier::BOLD),
            )
        } else if let Some(ref usage) = state.current_usage {
            Span::styled(
                format!("{} tokens", usage.total_tokens.unwrap_or_default()),
                Style::default().fg(Color::DarkGray),
            )
        } else {
            Span::styled("/ for commands", Style::default().fg(Color::DarkGray))
        },
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled("Ctrl+Y Copy", Style::default().fg(HadesTheme::RATATUI_GOLD)),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled("Ctrl+C exit", Style::default().fg(Color::DarkGray)),
    ]);

    let paragraph = Paragraph::new(status_line);
    frame.render_widget(paragraph, area);
}

/// Helper computing a centered popup rectangle given percentage dimensions.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Renders the floating Command Palette overlay.
fn render_command_palette(frame: &mut Frame, app: &HadesApp, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(60, 50, area);
    frame.render_widget(Clear, popup_area);

    let commands = app.commands().list();
    let items: Vec<ListItem> = commands
        .iter()
        .enumerate()
        .map(|(idx, cmd)| {
            let is_selected = idx == state.selected_palette_index;
            let style = if is_selected {
                Style::default()
                    .fg(HadesTheme::RATATUI_ORANGE)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if is_selected { " ▸ " } else { "   " };
            let line = Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(
                    format!("{:<12}", cmd.name),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {}", cmd.description), style),
            ]);

            ListItem::new(line)
        })
        .collect();

    let block = Block::default()
        .title(" Commands ")
        .title_style(
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(HadesTheme::RATATUI_ORANGE));

    let list = List::new(items).block(block);
    frame.render_widget(list, popup_area);
}

/// Renders the floating Session Switcher modal with relative timestamps and active indicators.
fn render_session_select(frame: &mut Frame, app: &HadesApp, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(82, 65, area);
    frame.render_widget(Clear, popup_area);

    let active_id = app.active_session().map(|a| a.metadata.id.clone());

    let items: Vec<ListItem> = if state.sessions.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "   No saved sessions found. Press Esc to return.",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        state
            .sessions
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let is_selected = idx == state.selected_session_index;
                let is_active = active_id.as_deref() == Some(&s.id);
                let time_display = hades_storage::format_session_timestamp(s.updated_at, is_active);

                let style = if is_selected {
                    Style::default()
                        .fg(HadesTheme::RATATUI_ORANGE)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let prefix = if is_selected { " ▸ " } else { "   " };
                let short_id = if s.id.len() >= 8 { &s.id[..8] } else { &s.id };
                let model_str = s.active_model.as_deref().unwrap_or("no model");

                let bullet_span = if is_active {
                    Span::styled(
                        "● ",
                        Style::default()
                            .fg(HadesTheme::RATATUI_GOLD)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    Span::styled("○ ", Style::default().fg(Color::DarkGray))
                };

                let time_span = if is_active {
                    Span::styled(
                        format!("  {time_display}"),
                        Style::default()
                            .fg(HadesTheme::RATATUI_GOLD)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    Span::styled(
                        format!("  {time_display}"),
                        Style::default().fg(Color::DarkGray),
                    )
                };

                let line = Line::from(vec![
                    Span::styled(prefix, style),
                    bullet_span,
                    Span::styled(
                        format!(
                            "{:<24}",
                            if s.title.len() > 24 {
                                format!("{}...", &s.title[..21])
                            } else {
                                s.title.clone()
                            }
                        ),
                        style.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" [{short_id}]"),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!(
                            " [{:<16}]",
                            if model_str.len() > 16 {
                                format!("{}...", &model_str[..13])
                            } else {
                                model_str.to_string()
                            }
                        ),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!(" {:>3} msgs", s.message_count),
                        Style::default().fg(Color::Green),
                    ),
                    time_span,
                ]);

                ListItem::new(line)
            })
            .collect()
    };

    let block = Block::default()
        .title(" Conversation Sessions  [Enter: Open | r: Rename | d: Delete | Esc: Back] ")
        .title_style(
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(HadesTheme::RATATUI_ORANGE));

    let list = List::new(items).block(block);
    frame.render_widget(list, popup_area);
}

/// Renders the floating Copy Mode modal for selecting conversation turns to copy.
fn render_copy_select(frame: &mut Frame, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(82, 65, area);
    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = if state.turns.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "   No conversation turns available to copy. Press Esc to return.",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        state
            .turns
            .iter()
            .enumerate()
            .map(|(idx, turn)| {
                let is_selected = idx == state.copy_selected_turn_index;
                let style = if is_selected {
                    Style::default()
                        .fg(HadesTheme::RATATUI_ORANGE)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let prefix = if is_selected { " ▸ " } else { "   " };
                let turn_num = idx + 1;

                let prompt_preview = if turn.user_prompt.len() > 36 {
                    format!("{}...", &turn.user_prompt[..33])
                } else {
                    turn.user_prompt.clone()
                };

                let resp_preview = if let Some(ref resp) = turn.assistant_response {
                    let first_line = resp.lines().next().unwrap_or("");
                    if first_line.len() > 36 {
                        format!("{}...", &first_line[..33])
                    } else {
                        first_line.to_string()
                    }
                } else if let Some(ref err) = turn.error_text {
                    format!("Error: {err}")
                } else {
                    "(generating...)".to_string()
                };

                let line = Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(
                        format!("[Turn {turn_num}] "),
                        Style::default().fg(HadesTheme::RATATUI_GOLD),
                    ),
                    Span::styled(
                        format!("You: {:<38} ", prompt_preview),
                        style.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("| Hades: {}", resp_preview),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);

                ListItem::new(line)
            })
            .collect()
    };

    let block = Block::default()
        .title(" Copy Turn to Clipboard  [↑/↓: Select | Enter / y: Copy Turn | a: Copy All | Esc: Back] ")
        .title_style(
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(HadesTheme::RATATUI_ORANGE));

    let list = List::new(items).block(block);
    frame.render_widget(list, popup_area);
}

/// Renders the floating Session Rename dialog.
fn render_session_rename(frame: &mut Frame, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(60, 25, area);
    frame.render_widget(Clear, popup_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Title prompt
            Constraint::Length(3), // Input box
            Constraint::Length(1), // Key hints
        ])
        .margin(1)
        .split(popup_area);

    let block = Block::default()
        .title(" Rename Session ")
        .title_style(
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(HadesTheme::RATATUI_ORANGE));
    frame.render_widget(block, popup_area);

    let prompt_p =
        Paragraph::new("Enter new session title:").style(Style::default().fg(Color::White));
    frame.render_widget(prompt_p, chunks[0]);

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(HadesTheme::RATATUI_GOLD));
    let input_p = Paragraph::new(format!("{}█", state.rename_input))
        .style(
            Style::default()
                .fg(HadesTheme::RATATUI_GOLD)
                .add_modifier(Modifier::BOLD),
        )
        .block(input_block);
    frame.render_widget(input_p, chunks[1]);

    let hints_p = Paragraph::new("[Enter] Confirm    [Esc] Cancel")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hints_p, chunks[2]);
}

/// Renders the floating Session Deletion Confirmation modal.
fn render_session_delete_confirm(frame: &mut Frame, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(55, 30, area);
    frame.render_widget(Clear, popup_area);

    let title_display = if state.delete_session_title.len() > 30 {
        format!("{}...", &state.delete_session_title[..27])
    } else {
        state.delete_session_title.clone()
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Question
            Constraint::Length(2), // Warning note
            Constraint::Length(3), // Action buttons
        ])
        .margin(1)
        .split(popup_area);

    let block = Block::default()
        .title(" Delete Session ")
        .title_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red));
    frame.render_widget(block, popup_area);

    let q_p = Paragraph::new(format!("Delete session \"{}\"?", title_display)).style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(q_p, chunks[0]);

    let warn_p = Paragraph::new("This action is permanent and cannot be undone.")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(warn_p, chunks[1]);

    let btn_style_del = if state.delete_confirm_action == 0 {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Red)
    };
    let btn_style_can = if state.delete_confirm_action == 1 {
        Style::default()
            .fg(Color::Black)
            .bg(HadesTheme::RATATUI_ORANGE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let btns_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(" [y] Delete ", btn_style_del),
        Span::raw("     "),
        Span::styled(" [n / Esc] Cancel ", btn_style_can),
    ]);
    let btns_p = Paragraph::new(btns_line);
    frame.render_widget(btns_p, chunks[2]);
}

/// Renders the floating Provider Selection modal.
fn render_provider_select(frame: &mut Frame, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(65, 55, area);
    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = state
        .providers
        .iter()
        .enumerate()
        .map(|(idx, p)| {
            let is_selected = idx == state.selected_provider_index;
            let style = if is_selected {
                Style::default()
                    .fg(HadesTheme::RATATUI_ORANGE)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if is_selected { " ▸ " } else { "   " };
            let (badge, badge_color) = if p.is_local {
                ("[Local]", Color::Green)
            } else {
                ("[Cloud]", HadesTheme::RATATUI_GOLD)
            };

            let line = Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(format!("{badge:<8}"), Style::default().fg(badge_color)),
                Span::styled(
                    format!("{:<18}", p.name),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", p.description),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            ListItem::new(line)
        })
        .collect();

    let block = Block::default()
        .title(" Select AI Provider / Engine ")
        .title_style(
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(HadesTheme::RATATUI_ORANGE));

    let list = List::new(items).block(block);
    frame.render_widget(list, popup_area);
}

/// Renders the floating Model Selection modal.
fn render_model_select(frame: &mut Frame, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(70, 60, area);
    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = state
        .models
        .iter()
        .enumerate()
        .map(|(idx, m)| {
            let is_selected = idx == state.selected_model_index;
            let style = if is_selected {
                Style::default()
                    .fg(HadesTheme::RATATUI_ORANGE)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if is_selected { " ▸ " } else { "   " };
            let line = Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(
                    format!("{:<28}", m.display_name),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" Context: {:<6}", m.context_window_display()),
                    style,
                ),
            ]);

            ListItem::new(line)
        })
        .collect();

    let block = Block::default()
        .title(" Select Model ")
        .title_style(
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(HadesTheme::RATATUI_ORANGE));

    let list = List::new(items).block(block);
    frame.render_widget(list, popup_area);
}

/// Renders the floating Model Information Details card.
fn render_model_info(frame: &mut Frame, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(65, 55, area);
    frame.render_widget(Clear, popup_area);

    let model = match state.selected_model {
        Some(ref m) => m,
        None => return,
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Model: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &model.display_name,
                Style::default()
                    .fg(HadesTheme::RATATUI_ORANGE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({})", model.id),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Provider: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &model.provider_id,
                Style::default().fg(HadesTheme::RATATUI_GOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Capabilities:",
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )),
    ];

    for (cap, cap_state) in model.capabilities.iter() {
        let (symbol, color) = match cap_state {
            hades_provider::CapabilityState::Supported => ("✓", Color::Green),
            hades_provider::CapabilityState::Unsupported => ("✗", Color::DarkGray),
            hades_provider::CapabilityState::Unknown => ("?", Color::DarkGray),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("    {} ", symbol),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(cap.to_string(), Style::default().fg(Color::White)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  Context Window: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            model.context_window_display(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "  [ Enter = Proceed ]",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("    [ Esc = Back ]", Style::default().fg(Color::DarkGray)),
    ]));

    let block = Block::default()
        .title(" Model Details ")
        .title_style(
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(HadesTheme::RATATUI_ORANGE));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup_area);
}

/// Renders the floating Credential Input modal.
fn render_credential_input(frame: &mut Frame, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(65, 45, area);
    frame.render_widget(Clear, popup_area);

    let masked_key: String = "*".repeat(state.credential_input.len());

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Enter authentication API key (credentials are stored securely locally):",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  API Key: ",
                Style::default()
                    .fg(HadesTheme::RATATUI_ORANGE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if masked_key.is_empty() {
                    "(paste or type key)▌".to_string()
                } else {
                    format!("{masked_key}▌")
                },
                Style::default().fg(HadesTheme::RATATUI_GOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  Endpoint override: ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                if state.custom_endpoint_input.is_empty() {
                    "(default)"
                } else {
                    &state.custom_endpoint_input
                },
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter = Connect   Tab = Toggle field   Esc = Back",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .title(" Credential Setup ")
        .title_style(
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(HadesTheme::RATATUI_ORANGE));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup_area);
}

/// Renders the floating Connection Verification card.
fn render_verifying(frame: &mut Frame, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(50, 30, area);
    frame.render_widget(Clear, popup_area);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("  {} ", state.spinner_char()),
                Style::default()
                    .fg(HadesTheme::RATATUI_GOLD)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Verifying provider access & model...",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Please wait...",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .title(" Connecting ")
        .title_style(
            Style::default()
                .fg(HadesTheme::RATATUI_GOLD)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(HadesTheme::RATATUI_GOLD));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, popup_area);
}

/// Renders the floating Verification Failed diagnostics dialog.
fn render_verification_failed(frame: &mut Frame, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(60, 50, area);
    frame.render_widget(Clear, popup_area);

    let error_msg = state
        .verification_error
        .as_deref()
        .unwrap_or("Authentication failed.");

    let actions = ["Retry", "Change Credential", "Back to Models"];
    let action_spans: Vec<Span> = actions
        .iter()
        .enumerate()
        .flat_map(|(idx, act)| {
            let is_sel = idx == state.verification_action_index;
            let style = if is_sel {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            vec![
                Span::styled(
                    if is_sel {
                        format!(" ▸ [{act}] ")
                    } else {
                        format!("   [{act}] ")
                    },
                    style,
                ),
                Span::raw("  "),
            ]
        })
        .collect();

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  Error: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(error_msg, Style::default().fg(Color::Red)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Please check API key validity and network connection.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(action_spans),
        Line::from(""),
        Line::from(Span::styled(
            "  ↑ ↓ Navigate   Enter Select   Esc Back",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .title(" Verification Failed ")
        .title_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup_area);
}

/// Renders the centered interactive modal requesting user authorization for tool execution.
fn render_tool_approval(frame: &mut Frame, app: &HadesApp, state: &TuiState, area: Rect) {
    let popup_area = centered_rect(70, 60, area);
    frame.render_widget(Clear, popup_area);

    let (call_name, risk_level, summary, details) = if let Some(req) = app.pending_approval() {
        (
            req.call.tool_name.as_str(),
            req.risk,
            req.summary.as_str(),
            req.details.as_str(),
        )
    } else {
        (
            "unknown",
            hades_tools::RiskLevel::Medium,
            "Tool execution authorization required.",
            "",
        )
    };

    let risk_color = match risk_level {
        hades_tools::RiskLevel::Safe => Color::Green,
        hades_tools::RiskLevel::Low => HadesTheme::RATATUI_GOLD,
        hades_tools::RiskLevel::Medium => Color::Yellow,
        hades_tools::RiskLevel::High => HadesTheme::RATATUI_FIRE,
        hades_tools::RiskLevel::Critical => Color::Red,
    };

    let block = Block::default()
        .title(Span::styled(
            " 🚨 ATTENTION REQUIRED: USER AUTHORIZATION NEEDED 🚨 ",
            Style::default()
                .fg(Color::White)
                .bg(HadesTheme::RATATUI_FIRE)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(HadesTheme::RATATUI_FIRE));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Risk & Tool Header
            Constraint::Length(2), // Summary
            Constraint::Min(3),    // Details
            Constraint::Length(3), // Action buttons
        ])
        .split(inner);

    // 1. Header with Risk Badge and Requesting Agent Badge
    let mut header_spans = vec![
        Span::styled("  Tool: ", Style::default().fg(Color::White)),
        Span::styled(
            call_name,
            Style::default()
                .fg(HadesTheme::RATATUI_ORANGE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("   Risk: ", Style::default().fg(Color::White)),
        Span::styled(
            format!(" [{risk_level}] "),
            Style::default()
                .fg(Color::Black)
                .bg(risk_color)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    if let Some(req) = app.pending_approval() {
        if let Some(ref role) = req.agent_role {
            header_spans.push(Span::styled(
                "   Agent: ",
                Style::default().fg(Color::White),
            ));
            header_spans.push(Span::styled(
                format!(" [{role}] "),
                Style::default()
                    .fg(Color::Black)
                    .bg(HadesTheme::RATATUI_CYAN)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(header_spans)), chunks[0]);

    // 2. Summary
    let summary_para = Paragraph::new(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            summary,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(summary_para, chunks[1]);

    // 3. Details
    let details_lines: Vec<Line> = details
        .lines()
        .map(|l| {
            Line::from(Span::styled(
                format!("  {l}"),
                Style::default().fg(Color::Gray),
            ))
        })
        .collect();
    let details_para = Paragraph::new(details_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " Invocation Details ",
                    Style::default().fg(Color::DarkGray),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(details_para, chunks[2]);

    // 4. Buttons
    let actions = ["Allow Once", "Allow for Session", "Deny", "Cancel"];
    let button_spans: Vec<Span> = actions
        .iter()
        .enumerate()
        .flat_map(|(i, &label)| {
            let is_selected = i == state.tool_approval_selection;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(HadesTheme::RATATUI_ORANGE)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            vec![
                Span::styled(format!(" [ {label} ] "), style),
                Span::styled("  ", Style::default()),
            ]
        })
        .collect();

    let button_line = Line::from(button_spans);
    let nav_hint = Line::from(Span::styled(
        "←/→ or Tab to select, Enter to confirm, y/s/d/Esc shortcuts",
        Style::default().fg(Color::DarkGray),
    ));

    let button_para = Paragraph::new(vec![button_line, nav_hint]).alignment(Alignment::Center);
    frame.render_widget(button_para, chunks[3]);
}

fn render_mcp_setup(frame: &mut Frame, state: &TuiState, area: Rect) {
    // Create a centered modal for MCP server setup
    let modal = Rect {
        x: area.x + area.width.saturating_sub(80) / 2,
        y: area.y + area.height.saturating_sub(20) / 2,
        width: 80,
        height: 20,
    };

    // Background
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Black).fg(Color::White))
            .title("Add MCP Server"),
        modal,
    );

    let inner = Rect {
        x: modal.x + 1,
        y: modal.y + 1,
        width: modal.width.saturating_sub(2),
        height: modal.height.saturating_sub(2),
    };

    // Split into field areas
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(vec![
            Constraint::Length(2), // Server name
            Constraint::Length(2), // Transport
            Constraint::Length(2), // Command/URL
            Constraint::Length(2), // Args
            Constraint::Length(2), // Secure token
            Constraint::Length(2), // Token environment variable
            Constraint::Min(0),    // Error message
        ])
        .split(inner);

    // Helper to render input field
    let render_field = |frame: &mut Frame,
                        idx: usize,
                        label: &str,
                        value: &str,
                        _cursor_pos: usize,
                        is_active: bool| {
        let style = if is_active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };

        let text = if is_active {
            format!("{} {}_", label, value)
        } else {
            format!("{} {}", label, value)
        };

        frame.render_widget(Paragraph::new(text).style(style), chunks[idx]);
    };

    // Render fields
    render_field(
        frame,
        0,
        "Name:",
        &state.mcp_server_name,
        state.mcp_server_cursor_position,
        state.mcp_current_field == 0,
    );

    // Transport selector
    let transport_text = format!(
        "Transport: [{}] {}",
        if state.mcp_transport_selection == 0 {
            "●"
        } else {
            "○"
        },
        if state.mcp_transport_selection == 0 {
            "STDIO"
        } else {
            "HTTP"
        }
    );
    let transport_style = if state.mcp_current_field == 1 {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    frame.render_widget(
        Paragraph::new(transport_text).style(transport_style),
        chunks[1],
    );

    // Command/URL field
    let cmd_label = if state.mcp_transport_selection == 0 {
        "Command:"
    } else {
        "URL:"
    };
    let cmd_value = if state.mcp_transport_selection == 0 {
        &state.mcp_command_input
    } else {
        &state.mcp_url_input
    };
    render_field(
        frame,
        2,
        cmd_label,
        cmd_value,
        if state.mcp_transport_selection == 0 {
            state.mcp_command_cursor_position
        } else {
            state.mcp_url_cursor_position
        },
        state.mcp_current_field == 2,
    );

    // Args field
    render_field(
        frame,
        3,
        "Args:",
        &state.mcp_args_input,
        state.mcp_args_cursor_position,
        state.mcp_current_field == 3,
    );

    // Secure token field. The value is masked before rendering.
    let masked_token = "*".repeat(state.mcp_auth_token_input.chars().count());
    render_field(
        frame,
        4,
        "Token (secure):",
        &masked_token,
        state.mcp_auth_token_cursor_position,
        state.mcp_current_field == 4,
    );

    // Token environment variable fallback.
    render_field(
        frame,
        5,
        "Token env:",
        &state.mcp_token_env_input,
        state.mcp_token_env_cursor_position,
        state.mcp_current_field == 5,
    );

    // Error message or help text
    let error_text = if let Some(err) = &state.mcp_setup_error {
        Span::styled(format!("✗ {}", err), Style::default().fg(Color::Red))
    } else {
        Span::styled(
            "Tab/↑↓ to navigate, Left/Right on Transport, Enter to save, Esc to cancel",
            Style::default().fg(Color::DarkGray),
        )
    };
    frame.render_widget(Paragraph::new(Line::from(error_text)), chunks[6]);
}
