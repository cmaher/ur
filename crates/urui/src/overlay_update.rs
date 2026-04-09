//! Overlay message handling for the v2 update loop.
//!
//! Each overlay open message sets `model.active_overlay`. Each
//! close/cancel/select message clears the overlay. Internal navigation messages
//! mutate the overlay state in-place.

use super::cmd::Cmd;
use super::components::create_action_menu::{action_at, action_count};
use super::components::filter_menu::{CATEGORIES, PRIORITY_OPTIONS, STATUS_OPTIONS, toggle_vec};
use super::components::goto_menu::resolve_goto_target;
use super::components::priority_picker::{cursor_to_priority, priority_count, priority_to_cursor};
use super::components::settings_overlay::{build_settings_state, selected_theme_name, snap_cursor};
use super::components::type_menu::{cursor_to_type, type_count, type_to_cursor};
use super::model::{ActiveOverlay, FilterCategory, Model, SettingsLevel};
use super::msg::{GotoTarget, Msg, NavMsg, OverlayMsg};
use super::navigation::{PageId, TabId};

/// Handle an overlay message, returning updated model and commands.
pub fn handle_overlay(model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::Consumed => (model, vec![]),

        // === Priority Picker ===
        OverlayMsg::OpenPriorityPicker { .. }
        | OverlayMsg::PriorityPickerNavigate { .. }
        | OverlayMsg::PriorityPickerConfirm
        | OverlayMsg::PriorityPickerQuickSelect { .. }
        | OverlayMsg::PrioritySelected { .. }
        | OverlayMsg::PriorityCancelled => handle_priority_overlay(model, msg),

        // === Type Menu ===
        OverlayMsg::OpenTypeMenu { .. }
        | OverlayMsg::TypeMenuNavigate { .. }
        | OverlayMsg::TypeMenuConfirm
        | OverlayMsg::TypeMenuQuickSelect { .. }
        | OverlayMsg::TypeSelected { .. }
        | OverlayMsg::TypeMenuCancelled => handle_type_overlay(model, msg),

        // === Filter Menu ===
        OverlayMsg::OpenFilterMenu
        | OverlayMsg::FilterMenuNavigate { .. }
        | OverlayMsg::FilterMenuActivate
        | OverlayMsg::FilterMenuQuickToggle { .. }
        | OverlayMsg::FilterMenuClosed => handle_filter_overlay(model, msg),

        // === Goto Menu ===
        OverlayMsg::OpenGotoMenu { .. }
        | OverlayMsg::GotoMenuNavigate { .. }
        | OverlayMsg::GotoMenuConfirm
        | OverlayMsg::GotoMenuQuickSelect { .. }
        | OverlayMsg::GotoSelected(_)
        | OverlayMsg::GotoCancelled => handle_goto_overlay(model, msg),

        // === Force Close Confirm ===
        OverlayMsg::OpenForceCloseConfirm { .. }
        | OverlayMsg::ForceCloseConfirmYes
        | OverlayMsg::ForceCloseConfirmed { .. }
        | OverlayMsg::ForceCloseCancelled => handle_force_close_overlay(model, msg),

        // === Create Action Menu ===
        OverlayMsg::OpenCreateActionMenu { .. }
        | OverlayMsg::CreateActionNavigate { .. }
        | OverlayMsg::CreateActionConfirm
        | OverlayMsg::CreateActionQuickSelect { .. }
        | OverlayMsg::CreateActionSelected(_) => handle_create_action_overlay(model, msg),

        // === Project Input ===
        OverlayMsg::OpenProjectInput
        | OverlayMsg::ProjectInputChar(_)
        | OverlayMsg::ProjectInputBackspace
        | OverlayMsg::ProjectInputSubmitRequest
        | OverlayMsg::ProjectInputSubmitted(_)
        | OverlayMsg::ProjectInputCancelled => handle_project_input_overlay(model, msg),

        // === Title Input ===
        OverlayMsg::OpenTitleInput
        | OverlayMsg::TitleInputChar(_)
        | OverlayMsg::TitleInputBackspace
        | OverlayMsg::TitleInputSubmitRequest
        | OverlayMsg::TitleInputSubmitted(_)
        | OverlayMsg::TitleInputCancelled => handle_title_input_overlay(model, msg),

        // === Branch Input ===
        OverlayMsg::OpenBranchInput { .. }
        | OverlayMsg::BranchInputChar(_)
        | OverlayMsg::BranchInputBackspace
        | OverlayMsg::BranchInputSubmitRequest
        | OverlayMsg::BranchInputSubmitted { .. }
        | OverlayMsg::BranchInputCancelled => handle_branch_input_overlay(model, msg),

        // === Settings Overlay ===
        OverlayMsg::OpenSettings { .. }
        | OverlayMsg::SettingsEsc
        | OverlayMsg::SettingsNavigate { .. }
        | OverlayMsg::SettingsActivate
        | OverlayMsg::ThemeSelected(_)
        | OverlayMsg::SettingsClosed => handle_settings_overlay(model, msg),

        // === Help Overlay ===
        OverlayMsg::OpenHelp | OverlayMsg::HelpClosed => handle_help_overlay(model, msg),
    }
}

/// Handle priority picker overlay messages.
fn handle_priority_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenPriorityPicker {
            ticket_id,
            current_priority,
        } => {
            let cursor = priority_to_cursor(current_priority);
            model.open_overlay(ActiveOverlay::PriorityPicker { ticket_id, cursor });
            (model, vec![])
        }
        OverlayMsg::PriorityPickerNavigate { delta } => {
            if let Some(ActiveOverlay::PriorityPicker { ref mut cursor, .. }) = model.active_overlay
            {
                let count = priority_count();
                if delta > 0 && *cursor < count - 1 {
                    *cursor += 1;
                } else if delta < 0 && *cursor > 0 {
                    *cursor -= 1;
                }
            }
            (model, vec![])
        }
        OverlayMsg::PriorityPickerConfirm => {
            if let Some(ActiveOverlay::PriorityPicker {
                ref ticket_id,
                cursor,
            }) = model.active_overlay
            {
                let priority = cursor_to_priority(cursor);
                let ticket_id = ticket_id.clone();
                model.close_overlay();
                return handle_overlay(
                    model,
                    OverlayMsg::PrioritySelected {
                        ticket_id,
                        priority,
                    },
                );
            }
            (model, vec![])
        }
        OverlayMsg::PriorityPickerQuickSelect { digit } => {
            if let Some(ActiveOverlay::PriorityPicker { ref ticket_id, .. }) = model.active_overlay
            {
                let ticket_id = ticket_id.clone();
                model.close_overlay();
                return handle_overlay(
                    model,
                    OverlayMsg::PrioritySelected {
                        ticket_id,
                        priority: digit,
                    },
                );
            }
            (model, vec![])
        }
        OverlayMsg::PrioritySelected {
            ticket_id,
            priority,
        } => {
            let op = super::msg::TicketOpMsg::SetPriority {
                ticket_id,
                priority,
            };
            super::update::update(model, super::msg::Msg::TicketOp(op))
        }
        OverlayMsg::PriorityCancelled => {
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Handle type menu overlay messages.
fn handle_type_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenTypeMenu {
            ticket_id,
            current_type,
        } => {
            let cursor = type_to_cursor(&current_type);
            model.open_overlay(ActiveOverlay::TypeMenu { ticket_id, cursor });
            (model, vec![])
        }
        OverlayMsg::TypeMenuNavigate { delta } => {
            if let Some(ActiveOverlay::TypeMenu { ref mut cursor, .. }) = model.active_overlay {
                let count = type_count();
                if delta > 0 && *cursor < count - 1 {
                    *cursor += 1;
                } else if delta < 0 && *cursor > 0 {
                    *cursor -= 1;
                }
            }
            (model, vec![])
        }
        OverlayMsg::TypeMenuConfirm => {
            if let Some(ActiveOverlay::TypeMenu {
                ref ticket_id,
                cursor,
            }) = model.active_overlay
            {
                let ticket_type = cursor_to_type(cursor).to_owned();
                let ticket_id = ticket_id.clone();
                model.close_overlay();
                return handle_overlay(
                    model,
                    OverlayMsg::TypeSelected {
                        ticket_id,
                        ticket_type,
                    },
                );
            }
            (model, vec![])
        }
        OverlayMsg::TypeMenuQuickSelect { index } => {
            if index < type_count()
                && let Some(ActiveOverlay::TypeMenu { ref ticket_id, .. }) = model.active_overlay
            {
                let ticket_id = ticket_id.clone();
                let ticket_type = cursor_to_type(index).to_owned();
                model.close_overlay();
                return handle_overlay(
                    model,
                    OverlayMsg::TypeSelected {
                        ticket_id,
                        ticket_type,
                    },
                );
            }
            (model, vec![])
        }
        OverlayMsg::TypeSelected {
            ticket_id,
            ticket_type,
        } => {
            let op = super::msg::TicketOpMsg::SetType {
                ticket_id,
                ticket_type,
            };
            super::update::update(model, super::msg::Msg::TicketOp(op))
        }
        OverlayMsg::TypeMenuCancelled => {
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Handle filter menu overlay messages.
fn handle_filter_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenFilterMenu => {
            model.open_overlay(ActiveOverlay::FilterMenu {
                cursor: 0,
                expanded: None,
                sub_cursor: 0,
            });
            (model, vec![])
        }
        OverlayMsg::FilterMenuNavigate { delta } => {
            handle_filter_navigate(&mut model, delta);
            (model, vec![])
        }
        OverlayMsg::FilterMenuActivate => {
            handle_filter_activate(&mut model);
            (model, vec![])
        }
        OverlayMsg::FilterMenuQuickToggle { digit } => {
            handle_filter_quick_toggle(&mut model, digit);
            (model, vec![])
        }
        OverlayMsg::FilterMenuClosed => {
            if let Some(ActiveOverlay::FilterMenu {
                ref mut expanded, ..
            }) = model.active_overlay
                && expanded.is_some()
            {
                *expanded = None;
                return (model, vec![]);
            }
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Handle goto menu overlay messages.
fn handle_goto_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenGotoMenu { targets } => {
            model.open_overlay(ActiveOverlay::GotoMenu { targets, cursor: 0 });
            (model, vec![])
        }
        OverlayMsg::GotoMenuNavigate { delta } => {
            if let Some(ActiveOverlay::GotoMenu {
                ref targets,
                ref mut cursor,
            }) = model.active_overlay
            {
                let count = targets.len();
                if delta > 0 && count > 0 && *cursor < count - 1 {
                    *cursor += 1;
                } else if delta < 0 && *cursor > 0 {
                    *cursor -= 1;
                }
            }
            (model, vec![])
        }
        OverlayMsg::GotoMenuConfirm => {
            if let Some(ActiveOverlay::GotoMenu {
                ref targets,
                cursor,
            }) = model.active_overlay
                && let Some(target) = resolve_goto_target(targets, cursor)
            {
                model.close_overlay();
                return handle_overlay(model, OverlayMsg::GotoSelected(target));
            }
            (model, vec![])
        }
        OverlayMsg::GotoMenuQuickSelect { digit } => {
            if let Some(ActiveOverlay::GotoMenu { ref targets, .. }) = model.active_overlay
                && digit >= 1
                && digit <= targets.len()
                && let Some(target) = resolve_goto_target(targets, digit - 1)
            {
                model.close_overlay();
                return handle_overlay(model, OverlayMsg::GotoSelected(target));
            }
            (model, vec![])
        }
        OverlayMsg::GotoSelected(target) => handle_goto_selected(model, target),
        OverlayMsg::GotoCancelled => {
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Handle a selected goto target by navigating to the appropriate page/tab.
fn handle_goto_selected(mut model: Model, target: GotoTarget) -> (Model, Vec<Cmd>) {
    match target.screen.as_str() {
        "ticket" => {
            let page = PageId::TicketDetail {
                ticket_id: target.id,
            };
            let mut nav = std::mem::replace(
                &mut model.navigation_model,
                super::navigation::NavigationModel::initial(),
            );
            let cmds = nav.push(page, &mut model);
            model.navigation_model = nav;
            (model, cmds)
        }
        "flow" => {
            // Switch to flows tab, then push flow detail
            let (mut model, mut cmds) =
                super::update::update(model, Msg::Nav(NavMsg::TabSwitch(TabId::Flows)));
            let page = PageId::FlowDetail {
                ticket_id: target.id,
            };
            let mut nav = std::mem::replace(
                &mut model.navigation_model,
                super::navigation::NavigationModel::initial(),
            );
            let push_cmds = nav.push(page, &mut model);
            model.navigation_model = nav;
            cmds.extend(push_cmds);
            (model, cmds)
        }
        "worker" => super::update::update(model, Msg::Nav(NavMsg::TabSwitch(TabId::Workers))),
        _ => (model, vec![]),
    }
}

/// Handle force close confirm overlay messages.
fn handle_force_close_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenForceCloseConfirm {
            ticket_id,
            open_children,
        } => {
            model.open_overlay(ActiveOverlay::ForceCloseConfirm {
                ticket_id,
                open_children,
            });
            (model, vec![])
        }
        OverlayMsg::ForceCloseConfirmYes => {
            if let Some(ActiveOverlay::ForceCloseConfirm { ref ticket_id, .. }) =
                model.active_overlay
            {
                let ticket_id = ticket_id.clone();
                model.close_overlay();
                return handle_overlay(model, OverlayMsg::ForceCloseConfirmed { ticket_id });
            }
            (model, vec![])
        }
        OverlayMsg::ForceCloseConfirmed { ticket_id } => {
            let op = super::msg::TicketOpMsg::ForceClose { ticket_id };
            super::update::update(model, super::msg::Msg::TicketOp(op))
        }
        OverlayMsg::ForceCloseCancelled => {
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Handle create action menu overlay messages.
fn handle_create_action_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenCreateActionMenu { pending } => {
            model.open_overlay(ActiveOverlay::CreateActionMenu { pending, cursor: 0 });
            (model, vec![])
        }
        OverlayMsg::CreateActionNavigate { delta } => {
            if let Some(ActiveOverlay::CreateActionMenu { ref mut cursor, .. }) =
                model.active_overlay
            {
                let count = action_count();
                if delta > 0 {
                    *cursor = (*cursor + 1) % count;
                } else if delta < 0 {
                    *cursor = (*cursor + count - 1) % count;
                }
            }
            (model, vec![])
        }
        OverlayMsg::CreateActionConfirm => {
            if let Some(ActiveOverlay::CreateActionMenu {
                cursor,
                ref pending,
            }) = model.active_overlay
                && let Some(action) = action_at(cursor)
            {
                let pending = pending.clone();
                model.close_overlay();
                return handle_create_action(model, action, pending);
            }
            (model, vec![])
        }
        OverlayMsg::CreateActionQuickSelect { index } => {
            if let Some(ActiveOverlay::CreateActionMenu { ref pending, .. }) = model.active_overlay
                && let Some(action) = action_at(index)
            {
                let pending = pending.clone();
                model.close_overlay();
                return handle_create_action(model, action, pending);
            }
            (model, vec![])
        }
        OverlayMsg::CreateActionSelected(action) => {
            // Esc sends Abandon here directly (without going through Confirm).
            // Close the overlay and discard the pending ticket.
            if matches!(action, super::msg::CreateAction::Abandon) {
                model.close_overlay();
            }
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Handle project input overlay messages.
///
/// These overlays are no longer used by the create ticket flow (which now uses
/// $EDITOR), but the handlers remain for backward compatibility.
fn handle_project_input_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenProjectInput => {
            model.open_overlay(ActiveOverlay::ProjectInput {
                buffer: String::new(),
            });
            (model, vec![])
        }
        OverlayMsg::ProjectInputChar(c) => {
            if let Some(ActiveOverlay::ProjectInput { ref mut buffer }) = model.active_overlay {
                buffer.push(c);
            }
            (model, vec![])
        }
        OverlayMsg::ProjectInputBackspace => {
            if let Some(ActiveOverlay::ProjectInput { ref mut buffer }) = model.active_overlay {
                buffer.pop();
            }
            (model, vec![])
        }
        OverlayMsg::ProjectInputSubmitRequest => {
            if let Some(ActiveOverlay::ProjectInput { .. }) = model.active_overlay {
                model.close_overlay();
            }
            (model, vec![])
        }
        OverlayMsg::ProjectInputSubmitted(_) => (model, vec![]),
        OverlayMsg::ProjectInputCancelled => {
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Handle title input overlay messages.
///
/// These overlays are no longer used by the create ticket flow (which now uses
/// $EDITOR), but the handlers remain for backward compatibility.
fn handle_title_input_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenTitleInput => {
            model.open_overlay(ActiveOverlay::TitleInput {
                buffer: String::new(),
            });
            (model, vec![])
        }
        OverlayMsg::TitleInputChar(c) => {
            if let Some(ActiveOverlay::TitleInput { ref mut buffer }) = model.active_overlay {
                buffer.push(c);
            }
            (model, vec![])
        }
        OverlayMsg::TitleInputBackspace => {
            if let Some(ActiveOverlay::TitleInput { ref mut buffer }) = model.active_overlay {
                buffer.pop();
            }
            (model, vec![])
        }
        OverlayMsg::TitleInputSubmitRequest => {
            if let Some(ActiveOverlay::TitleInput { .. }) = model.active_overlay {
                model.close_overlay();
            }
            (model, vec![])
        }
        OverlayMsg::TitleInputSubmitted(_) => (model, vec![]),
        OverlayMsg::TitleInputCancelled => {
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Handle settings overlay messages.
fn handle_settings_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenSettings { custom_theme_names } => {
            // Merge explicit names with model's custom theme names from config.
            let names = if custom_theme_names.is_empty() {
                model.custom_theme_names.clone()
            } else {
                custom_theme_names
            };
            model.open_overlay(build_settings_state(names));
            (model, vec![])
        }
        OverlayMsg::SettingsEsc => handle_settings_esc(model),
        OverlayMsg::SettingsNavigate { direction } => {
            handle_settings_navigate(&mut model, direction);
            (model, vec![])
        }
        OverlayMsg::SettingsActivate => handle_settings_activate(model),
        OverlayMsg::ThemeSelected(name) => {
            // Apply the theme immediately by setting pending_theme_swap,
            // and persist the selection to ur.toml.
            model.pending_theme_swap = Some(name.clone());
            let cmd = Cmd::PersistTheme { theme_name: name };
            (model, vec![cmd])
        }
        OverlayMsg::SettingsClosed => {
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Handle help overlay messages.
/// Handle branch input overlay messages.
///
/// Opens a text input pre-filled with the current branch. On submit, fires
/// a SetBranch ticket operation (empty string clears the branch).
fn handle_branch_input_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenBranchInput {
            ticket_id,
            current_branch,
        } => {
            model.open_overlay(ActiveOverlay::BranchInput {
                buffer: current_branch,
                ticket_id,
            });
            (model, vec![])
        }
        OverlayMsg::BranchInputChar(c) => {
            if let Some(ActiveOverlay::BranchInput { ref mut buffer, .. }) = model.active_overlay {
                buffer.push(c);
            }
            (model, vec![])
        }
        OverlayMsg::BranchInputBackspace => {
            if let Some(ActiveOverlay::BranchInput { ref mut buffer, .. }) = model.active_overlay {
                buffer.pop();
            }
            (model, vec![])
        }
        OverlayMsg::BranchInputSubmitRequest => {
            if let Some(ActiveOverlay::BranchInput {
                ref buffer,
                ref ticket_id,
            }) = model.active_overlay
            {
                let ticket_id = ticket_id.clone();
                let branch = buffer.clone();
                model.close_overlay();
                let op = super::msg::TicketOpMsg::SetBranch { ticket_id, branch };
                return super::update::update(model, super::msg::Msg::TicketOp(op));
            }
            (model, vec![])
        }
        OverlayMsg::BranchInputSubmitted { .. } => (model, vec![]),
        OverlayMsg::BranchInputCancelled => {
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

fn handle_help_overlay(mut model: Model, msg: OverlayMsg) -> (Model, Vec<Cmd>) {
    match msg {
        OverlayMsg::OpenHelp => {
            model.open_overlay(ActiveOverlay::Help);
            (model, vec![])
        }
        OverlayMsg::HelpClosed => {
            model.close_overlay();
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

// === Filter Menu helpers ===

fn handle_filter_navigate(model: &mut Model, delta: i32) {
    if let Some(ActiveOverlay::FilterMenu {
        ref mut cursor,
        ref expanded,
        ref mut sub_cursor,
    }) = model.active_overlay
    {
        if let Some(cat) = expanded {
            let count = filter_submenu_count(*cat, &model.ticket_filters);
            if delta > 0 && count > 0 && *sub_cursor < count - 1 {
                *sub_cursor += 1;
            } else if delta < 0 && *sub_cursor > 0 {
                *sub_cursor -= 1;
            }
        } else if delta > 0 && *cursor < CATEGORIES.len() - 1 {
            *cursor += 1;
        } else if delta < 0 && *cursor > 0 {
            *cursor -= 1;
        }
    }
}

fn handle_filter_activate(model: &mut Model) {
    // We need to extract values to avoid multiple mutable borrows.
    let (expanded, cursor) = match &model.active_overlay {
        Some(ActiveOverlay::FilterMenu {
            expanded, cursor, ..
        }) => (*expanded, *cursor),
        _ => return,
    };

    if let Some(cat) = expanded {
        let sub_cursor = match &model.active_overlay {
            Some(ActiveOverlay::FilterMenu { sub_cursor, .. }) => *sub_cursor,
            _ => return,
        };
        toggle_filter_item(cat, sub_cursor, model);
    } else {
        let cat = CATEGORIES[cursor];
        if cat == FilterCategory::ShowChildren {
            model.ticket_filters.show_children = !model.ticket_filters.show_children;
        } else if let Some(ActiveOverlay::FilterMenu {
            ref mut expanded,
            ref mut sub_cursor,
            ..
        }) = model.active_overlay
        {
            *expanded = Some(cat);
            *sub_cursor = 0;
        }
    }
}

fn handle_filter_quick_toggle(model: &mut Model, c: char) {
    let expanded = match &model.active_overlay {
        Some(ActiveOverlay::FilterMenu { expanded, .. }) => *expanded,
        _ => return,
    };

    if expanded.is_none() {
        let digit = (c as u8 - b'0') as usize;
        if digit >= 1 && digit <= CATEGORIES.len() {
            // Set cursor and activate
            if let Some(ActiveOverlay::FilterMenu { ref mut cursor, .. }) = model.active_overlay {
                *cursor = digit - 1;
            }
            handle_filter_activate(model);
        }
        return;
    }

    let Some(cat) = expanded else { return };
    let digit = (c as u8 - b'0') as usize;
    let index = match cat {
        FilterCategory::Priority => {
            if digit <= 4 {
                digit
            } else {
                return;
            }
        }
        _ => {
            if digit == 0 {
                return;
            }
            digit - 1
        }
    };
    let count = filter_submenu_count(cat, &model.ticket_filters);
    if index < count {
        toggle_filter_item(cat, index, model);
    }
}

fn toggle_filter_item(cat: FilterCategory, index: usize, model: &mut Model) {
    match cat {
        FilterCategory::Status => {
            let value = STATUS_OPTIONS[index].to_string();
            toggle_vec(&mut model.ticket_filters.statuses, value);
        }
        FilterCategory::Priority => {
            let value = PRIORITY_OPTIONS[index];
            toggle_vec(&mut model.ticket_filters.priorities, value);
        }
        FilterCategory::Project => {
            // We don't have project names in the model — they come from ctx.
            // For now, this is a no-op. When pages integrate, they will supply
            // project names through the overlay's open message.
        }
        FilterCategory::ShowChildren => {}
    }
}

fn filter_submenu_count(cat: FilterCategory, _filters: &super::model::TicketFilters) -> usize {
    match cat {
        FilterCategory::Status => STATUS_OPTIONS.len(),
        FilterCategory::Priority => PRIORITY_OPTIONS.len(),
        FilterCategory::Project => 0, // Populated from ctx at render time
        FilterCategory::ShowChildren => 0,
    }
}

// === Settings helpers ===

fn handle_settings_esc(mut model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ActiveOverlay::Settings { ref mut level, .. }) = model.active_overlay {
        match level {
            SettingsLevel::ThemePicker => {
                *level = SettingsLevel::TopLevel;
                return (model, vec![]);
            }
            SettingsLevel::TopLevel => {
                model.close_overlay();
                return (model, vec![]);
            }
        }
    }
    model.close_overlay();
    (model, vec![])
}

fn handle_settings_navigate(model: &mut Model, direction: super::msg::SettingsDirection) {
    use super::msg::SettingsDirection;

    if let Some(ActiveOverlay::Settings {
        ref level,
        ref mut active_column,
        ref mut column_cursors,
        ref light_themes,
        ref dark_themes,
        ref custom_themes,
        ..
    }) = model.active_overlay
    {
        if *level != SettingsLevel::ThemePicker {
            return;
        }

        match direction {
            SettingsDirection::Left => {
                if *active_column > 0 {
                    *active_column -= 1;
                    let count =
                        column_item_count(*active_column, light_themes, dark_themes, custom_themes);
                    column_cursors[*active_column] =
                        snap_cursor(column_cursors[*active_column], count);
                }
            }
            SettingsDirection::Right => {
                if *active_column < 2 {
                    *active_column += 1;
                    let count =
                        column_item_count(*active_column, light_themes, dark_themes, custom_themes);
                    column_cursors[*active_column] =
                        snap_cursor(column_cursors[*active_column], count);
                }
            }
            SettingsDirection::Down => {
                let count =
                    column_item_count(*active_column, light_themes, dark_themes, custom_themes);
                if count > 0 && column_cursors[*active_column] < count - 1 {
                    column_cursors[*active_column] += 1;
                }
            }
            SettingsDirection::Up => {
                if column_cursors[*active_column] > 0 {
                    column_cursors[*active_column] -= 1;
                }
            }
        }
    }
}

fn column_item_count(col: usize, light: &[String], dark: &[String], custom: &[String]) -> usize {
    match col {
        0 => light.len(),
        1 => dark.len(),
        2 => custom.len(),
        _ => 0,
    }
}

fn handle_settings_activate(mut model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ActiveOverlay::Settings {
        ref mut level,
        active_column,
        column_cursors,
        ref light_themes,
        ref dark_themes,
        ref custom_themes,
        ..
    }) = model.active_overlay
    {
        match level {
            SettingsLevel::TopLevel => {
                *level = SettingsLevel::ThemePicker;
                return (model, vec![]);
            }
            SettingsLevel::ThemePicker => {
                if let Some(name) = selected_theme_name(
                    active_column,
                    column_cursors,
                    light_themes,
                    dark_themes,
                    custom_themes,
                ) {
                    // Don't close the overlay — user can keep browsing themes.
                    return handle_overlay(model, OverlayMsg::ThemeSelected(name));
                }
            }
        }
    }
    (model, vec![])
}

/// Handle a create action selection with the pending ticket data.
///
/// Maps each `CreateAction` variant to the corresponding `TicketOpMsg`.
/// The `Abandon` action is a no-op (the ticket is simply discarded).
fn handle_create_action(
    model: Model,
    action: super::msg::CreateAction,
    pending: super::msg::PendingTicket,
) -> (Model, Vec<Cmd>) {
    use super::msg::{CreateAction, TicketOpMsg};

    let op = match action {
        CreateAction::Create => TicketOpMsg::Create { pending },
        CreateAction::Dispatch => {
            // For dispatch, we need project_key and image_id. These are derived
            // from the pending's project field. The cmd_runner resolves the actual
            // image from server config; we pass what we have.
            let project_key = pending.project.clone();
            TicketOpMsg::CreateAndDispatch {
                pending,
                project_key,
                image_id: String::new(),
            }
        }
        CreateAction::Edit => {
            let content = crate::create_ticket::serialize_to_template(
                &pending.project,
                &pending.title,
                &pending.ticket_type,
                pending.priority,
                pending.branch.as_deref(),
                &pending.body,
            );
            return (
                model,
                vec![Cmd::SpawnEditor {
                    parent_id: pending.parent_id,
                    project: Some(pending.project),
                    content: Some(content),
                }],
            );
        }
        CreateAction::Abandon => return (model, vec![]),
    };

    super::update::update(model, super::msg::Msg::TicketOp(op))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::msg::{GotoTarget, OverlayMsg, PendingTicket, SettingsDirection};

    #[test]
    fn consumed_is_noop() {
        let model = Model::initial();
        let (new_model, cmds) = handle_overlay(model, OverlayMsg::Consumed);
        assert!(new_model.active_overlay.is_none());
        assert!(cmds.is_empty());
    }

    // === Priority Picker ===

    #[test]
    fn open_priority_picker_sets_state() {
        let model = Model::initial();
        let (new_model, _) = handle_overlay(
            model,
            OverlayMsg::OpenPriorityPicker {
                ticket_id: "ur-abc".into(),
                current_priority: 2,
            },
        );
        assert!(new_model.active_overlay.is_some());
        match &new_model.active_overlay {
            Some(ActiveOverlay::PriorityPicker { ticket_id, cursor }) => {
                assert_eq!(ticket_id, "ur-abc");
                assert_eq!(*cursor, 2);
            }
            _ => panic!("expected PriorityPicker"),
        }
    }

    #[test]
    fn priority_navigate_down() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenPriorityPicker {
                ticket_id: "ur-abc".into(),
                current_priority: 0,
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::PriorityPickerNavigate { delta: 1 });
        match &new_model.active_overlay {
            Some(ActiveOverlay::PriorityPicker { cursor, .. }) => assert_eq!(*cursor, 1),
            _ => panic!("expected PriorityPicker"),
        }
    }

    #[test]
    fn priority_navigate_up_no_underflow() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenPriorityPicker {
                ticket_id: "ur-abc".into(),
                current_priority: 0,
            },
        );
        let (new_model, _) =
            handle_overlay(model, OverlayMsg::PriorityPickerNavigate { delta: -1 });
        match &new_model.active_overlay {
            Some(ActiveOverlay::PriorityPicker { cursor, .. }) => assert_eq!(*cursor, 0),
            _ => panic!("expected PriorityPicker"),
        }
    }

    #[test]
    fn priority_confirm_closes_and_selects() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenPriorityPicker {
                ticket_id: "ur-abc".into(),
                current_priority: 2,
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::PriorityPickerConfirm);
        assert!(new_model.active_overlay.is_none());
    }

    #[test]
    fn priority_quick_select_closes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenPriorityPicker {
                ticket_id: "ur-abc".into(),
                current_priority: 0,
            },
        );
        let (new_model, _) =
            handle_overlay(model, OverlayMsg::PriorityPickerQuickSelect { digit: 3 });
        assert!(new_model.active_overlay.is_none());
    }

    #[test]
    fn priority_cancelled_closes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenPriorityPicker {
                ticket_id: "ur-abc".into(),
                current_priority: 0,
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::PriorityCancelled);
        assert!(new_model.active_overlay.is_none());
    }

    // === Filter Menu ===

    #[test]
    fn open_filter_menu_sets_state() {
        let model = Model::initial();
        let (new_model, _) = handle_overlay(model, OverlayMsg::OpenFilterMenu);
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::FilterMenu { .. })
        ));
    }

    #[test]
    fn filter_navigate_and_activate_show_children() {
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenFilterMenu);
        // Navigate to ShowChildren (index 3)
        let (model, _) = handle_overlay(model, OverlayMsg::FilterMenuNavigate { delta: 1 });
        let (model, _) = handle_overlay(model, OverlayMsg::FilterMenuNavigate { delta: 1 });
        let (model, _) = handle_overlay(model, OverlayMsg::FilterMenuNavigate { delta: 1 });
        assert!(!model.ticket_filters.show_children);
        let (new_model, _) = handle_overlay(model, OverlayMsg::FilterMenuActivate);
        assert!(new_model.ticket_filters.show_children);
    }

    #[test]
    fn filter_esc_collapses_submenu_first() {
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenFilterMenu);
        // Activate to expand Status submenu
        let (model, _) = handle_overlay(model, OverlayMsg::FilterMenuActivate);
        match &model.active_overlay {
            Some(ActiveOverlay::FilterMenu { expanded, .. }) => {
                assert!(expanded.is_some());
            }
            _ => panic!("expected FilterMenu"),
        }
        // Esc should collapse, not close
        let (new_model, _) = handle_overlay(model, OverlayMsg::FilterMenuClosed);
        assert!(new_model.active_overlay.is_some());
        match &new_model.active_overlay {
            Some(ActiveOverlay::FilterMenu { expanded, .. }) => {
                assert!(expanded.is_none());
            }
            _ => panic!("expected FilterMenu"),
        }
    }

    #[test]
    fn filter_esc_closes_when_not_expanded() {
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenFilterMenu);
        let (new_model, _) = handle_overlay(model, OverlayMsg::FilterMenuClosed);
        assert!(new_model.active_overlay.is_none());
    }

    // === Goto Menu ===

    #[test]
    fn open_goto_menu_sets_state() {
        let targets = vec![GotoTarget {
            label: "Test".into(),
            screen: "test".into(),
            id: "id-1".into(),
        }];
        let model = Model::initial();
        let (new_model, _) = handle_overlay(model, OverlayMsg::OpenGotoMenu { targets });
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::GotoMenu { .. })
        ));
    }

    #[test]
    fn goto_confirm_closes() {
        let targets = vec![GotoTarget {
            label: "Test".into(),
            screen: "test".into(),
            id: "id-1".into(),
        }];
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenGotoMenu { targets });
        let (new_model, _) = handle_overlay(model, OverlayMsg::GotoMenuConfirm);
        assert!(new_model.active_overlay.is_none());
    }

    #[test]
    fn goto_quick_select_closes() {
        let targets = vec![
            GotoTarget {
                label: "A".into(),
                screen: "a".into(),
                id: "1".into(),
            },
            GotoTarget {
                label: "B".into(),
                screen: "b".into(),
                id: "2".into(),
            },
        ];
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenGotoMenu { targets });
        let (new_model, _) = handle_overlay(model, OverlayMsg::GotoMenuQuickSelect { digit: 2 });
        assert!(new_model.active_overlay.is_none());
    }

    #[test]
    fn goto_cancelled_closes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenGotoMenu { targets: vec![] });
        let (new_model, _) = handle_overlay(model, OverlayMsg::GotoCancelled);
        assert!(new_model.active_overlay.is_none());
    }

    // === Force Close Confirm ===

    #[test]
    fn open_force_close_sets_state() {
        let model = Model::initial();
        let (new_model, _) = handle_overlay(
            model,
            OverlayMsg::OpenForceCloseConfirm {
                ticket_id: "ur-abc".into(),
                open_children: 3,
            },
        );
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::ForceCloseConfirm { .. })
        ));
    }

    #[test]
    fn force_close_confirm_yes_closes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenForceCloseConfirm {
                ticket_id: "ur-abc".into(),
                open_children: 3,
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::ForceCloseConfirmYes);
        assert!(new_model.active_overlay.is_none());
    }

    #[test]
    fn force_close_cancelled_closes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenForceCloseConfirm {
                ticket_id: "ur-abc".into(),
                open_children: 3,
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::ForceCloseCancelled);
        assert!(new_model.active_overlay.is_none());
    }

    // === Create Action Menu ===

    #[test]
    fn open_create_action_sets_state() {
        let model = Model::initial();
        let (new_model, _) = handle_overlay(
            model,
            OverlayMsg::OpenCreateActionMenu {
                pending: PendingTicket {
                    project: "ur".into(),
                    title: "Test".into(),
                    ticket_type: "code".into(),
                    priority: 2,
                    body: String::new(),
                    parent_id: None,
                    branch: None,
                },
            },
        );
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::CreateActionMenu { .. })
        ));
    }

    #[test]
    fn create_action_navigate_wraps() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenCreateActionMenu {
                pending: PendingTicket {
                    project: "ur".into(),
                    title: "Test".into(),
                    ticket_type: "code".into(),
                    priority: 2,
                    body: String::new(),
                    parent_id: None,
                    branch: None,
                },
            },
        );
        // Navigate up from 0 should wrap to last
        let (new_model, _) = handle_overlay(model, OverlayMsg::CreateActionNavigate { delta: -1 });
        match &new_model.active_overlay {
            Some(ActiveOverlay::CreateActionMenu { cursor, .. }) => {
                assert_eq!(*cursor, action_count() - 1);
            }
            _ => panic!("expected CreateActionMenu"),
        }
    }

    #[test]
    fn create_action_quick_select_closes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenCreateActionMenu {
                pending: PendingTicket {
                    project: "ur".into(),
                    title: "Test".into(),
                    ticket_type: "code".into(),
                    priority: 2,
                    body: String::new(),
                    parent_id: None,
                    branch: None,
                },
            },
        );
        let (new_model, _) =
            handle_overlay(model, OverlayMsg::CreateActionQuickSelect { index: 0 });
        assert!(new_model.active_overlay.is_none());
    }

    // === Project Input ===

    #[test]
    fn open_project_input_sets_state() {
        let model = Model::initial();
        let (new_model, _) = handle_overlay(model, OverlayMsg::OpenProjectInput);
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::ProjectInput { .. })
        ));
    }

    #[test]
    fn project_input_char_appends() {
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenProjectInput);
        let (model, _) = handle_overlay(model, OverlayMsg::ProjectInputChar('u'));
        let (new_model, _) = handle_overlay(model, OverlayMsg::ProjectInputChar('r'));
        match &new_model.active_overlay {
            Some(ActiveOverlay::ProjectInput { buffer }) => assert_eq!(buffer, "ur"),
            _ => panic!("expected ProjectInput"),
        }
    }

    #[test]
    fn project_input_backspace_deletes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenProjectInput);
        let (model, _) = handle_overlay(model, OverlayMsg::ProjectInputChar('a'));
        let (model, _) = handle_overlay(model, OverlayMsg::ProjectInputChar('b'));
        let (new_model, _) = handle_overlay(model, OverlayMsg::ProjectInputBackspace);
        match &new_model.active_overlay {
            Some(ActiveOverlay::ProjectInput { buffer }) => assert_eq!(buffer, "a"),
            _ => panic!("expected ProjectInput"),
        }
    }

    #[test]
    fn project_input_submit_closes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenProjectInput);
        let (model, _) = handle_overlay(model, OverlayMsg::ProjectInputChar('u'));
        let (new_model, _) = handle_overlay(model, OverlayMsg::ProjectInputSubmitRequest);
        assert!(new_model.active_overlay.is_none());
    }

    #[test]
    fn project_input_cancelled_closes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenProjectInput);
        let (new_model, _) = handle_overlay(model, OverlayMsg::ProjectInputCancelled);
        assert!(new_model.active_overlay.is_none());
    }

    // === Settings Overlay ===

    #[test]
    fn open_settings_sets_state() {
        let model = Model::initial();
        let (new_model, _) = handle_overlay(
            model,
            OverlayMsg::OpenSettings {
                custom_theme_names: vec![],
            },
        );
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::Settings { .. })
        ));
    }

    #[test]
    fn settings_esc_at_top_level_closes() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenSettings {
                custom_theme_names: vec![],
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::SettingsEsc);
        assert!(new_model.active_overlay.is_none());
    }

    #[test]
    fn settings_activate_enters_theme_picker() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenSettings {
                custom_theme_names: vec![],
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::SettingsActivate);
        match &new_model.active_overlay {
            Some(ActiveOverlay::Settings { level, .. }) => {
                assert_eq!(*level, SettingsLevel::ThemePicker);
            }
            _ => panic!("expected Settings"),
        }
    }

    #[test]
    fn settings_esc_at_theme_picker_returns_to_top() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenSettings {
                custom_theme_names: vec![],
            },
        );
        let (model, _) = handle_overlay(model, OverlayMsg::SettingsActivate);
        let (new_model, _) = handle_overlay(model, OverlayMsg::SettingsEsc);
        match &new_model.active_overlay {
            Some(ActiveOverlay::Settings { level, .. }) => {
                assert_eq!(*level, SettingsLevel::TopLevel);
            }
            _ => panic!("expected Settings"),
        }
    }

    #[test]
    fn settings_navigate_in_theme_picker() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenSettings {
                custom_theme_names: vec![],
            },
        );
        let (model, _) = handle_overlay(model, OverlayMsg::SettingsActivate);
        // Navigate down
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::SettingsNavigate {
                direction: SettingsDirection::Down,
            },
        );
        match &model.active_overlay {
            Some(ActiveOverlay::Settings { column_cursors, .. }) => {
                assert_eq!(column_cursors[0], 1)
            }
            _ => panic!("expected Settings"),
        }
        // Navigate right
        let (new_model, _) = handle_overlay(
            model,
            OverlayMsg::SettingsNavigate {
                direction: SettingsDirection::Right,
            },
        );
        match &new_model.active_overlay {
            Some(ActiveOverlay::Settings { active_column, .. }) => assert_eq!(*active_column, 1),
            _ => panic!("expected Settings"),
        }
    }

    // === Type Menu ===

    #[test]
    fn open_type_menu_sets_state() {
        let model = Model::initial();
        let (new_model, _) = handle_overlay(
            model,
            OverlayMsg::OpenTypeMenu {
                ticket_id: "ur-abc".into(),
                current_type: "design".into(),
            },
        );
        assert!(new_model.active_overlay.is_some());
        match &new_model.active_overlay {
            Some(ActiveOverlay::TypeMenu { ticket_id, cursor }) => {
                assert_eq!(ticket_id, "ur-abc");
                assert_eq!(*cursor, 1);
            }
            _ => panic!("expected TypeMenu"),
        }
    }

    #[test]
    fn type_navigate_down() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenTypeMenu {
                ticket_id: "ur-abc".into(),
                current_type: "code".into(),
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::TypeMenuNavigate { delta: 1 });
        match &new_model.active_overlay {
            Some(ActiveOverlay::TypeMenu { cursor, .. }) => assert_eq!(*cursor, 1),
            _ => panic!("expected TypeMenu"),
        }
    }

    #[test]
    fn type_navigate_up_clamps_at_zero() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenTypeMenu {
                ticket_id: "ur-abc".into(),
                current_type: "code".into(),
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::TypeMenuNavigate { delta: -1 });
        match &new_model.active_overlay {
            Some(ActiveOverlay::TypeMenu { cursor, .. }) => assert_eq!(*cursor, 0),
            _ => panic!("expected TypeMenu"),
        }
    }

    #[test]
    fn type_confirm_closes_overlay() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenTypeMenu {
                ticket_id: "ur-abc".into(),
                current_type: "code".into(),
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::TypeMenuConfirm);
        assert!(new_model.active_overlay.is_none());
    }

    #[test]
    fn type_quick_select_closes_overlay() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenTypeMenu {
                ticket_id: "ur-abc".into(),
                current_type: "code".into(),
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::TypeMenuQuickSelect { index: 1 });
        assert!(new_model.active_overlay.is_none());
    }

    // === open/close overlay state ===

    #[test]
    fn create_action_open_close_sets_and_clears_overlay() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenCreateActionMenu {
                pending: PendingTicket {
                    project: "ur".into(),
                    title: "Test".into(),
                    ticket_type: "code".into(),
                    priority: 2,
                    body: String::new(),
                    parent_id: None,
                    branch: None,
                },
            },
        );
        assert!(matches!(
            model.active_overlay,
            Some(ActiveOverlay::CreateActionMenu { .. })
        ));

        // Abandon path closes the overlay.
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::CreateActionSelected(crate::msg::CreateAction::Abandon),
        );
        assert!(model.active_overlay.is_none());
    }

    #[test]
    fn settings_open_close_sets_and_clears_overlay() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenSettings {
                custom_theme_names: vec![],
            },
        );
        assert!(matches!(
            model.active_overlay,
            Some(ActiveOverlay::Settings { .. })
        ));

        let (model, _) = handle_overlay(model, OverlayMsg::SettingsClosed);
        assert!(model.active_overlay.is_none());
    }

    #[test]
    fn filter_menu_open_close_sets_and_clears_overlay() {
        let model = Model::initial();
        let (model, _) = handle_overlay(model, OverlayMsg::OpenFilterMenu);
        assert!(matches!(
            model.active_overlay,
            Some(ActiveOverlay::FilterMenu { .. })
        ));

        // Closing from a non-expanded state should fully close.
        let (model, _) = handle_overlay(model, OverlayMsg::FilterMenuClosed);
        assert!(model.active_overlay.is_none());
    }

    #[test]
    fn type_cancel_closes_overlay() {
        let model = Model::initial();
        let (model, _) = handle_overlay(
            model,
            OverlayMsg::OpenTypeMenu {
                ticket_id: "ur-abc".into(),
                current_type: "code".into(),
            },
        );
        let (new_model, _) = handle_overlay(model, OverlayMsg::TypeMenuCancelled);
        assert!(new_model.active_overlay.is_none());
    }
}
