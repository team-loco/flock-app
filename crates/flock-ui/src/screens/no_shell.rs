use gpui::{App, Window};

use crate::kit::{Button, ConfirmDialog, EmptyState};

pub const DEBUG_IMAGE: &str = "busybox:1.36";
const NO_SHELL: &str = "This container has no shell";
const NO_SHELL_DETAIL: &str = "Its image has no /bin/sh. A debug container brings its own shell and sees this container’s processes.";
const START_DEBUG: &str = "Start debug container";

pub fn debug_body(pod: &str, container: &str) -> String {
    format!(
        "Adds an ephemeral {DEBUG_IMAGE} container to {pod} that targets {container}, then opens its shell. Ephemeral containers stay in the pod until it is deleted."
    )
}

pub fn no_shell_state(id: &'static str, start: Button) -> EmptyState {
    EmptyState::new(NO_SHELL)
        .selector(id)
        .detail(NO_SHELL_DETAIL)
        .action(start)
}

pub fn start_debug_button(id: &'static str) -> Button {
    Button::new(id, START_DEBUG).small().primary()
}

pub fn confirm_debug(
    pod: &str,
    container: &str,
    on_confirm: impl Fn(&mut Window, &mut App) + 'static,
) -> ConfirmDialog {
    ConfirmDialog::new(
        START_DEBUG,
        debug_body(pod, container),
        START_DEBUG,
        on_confirm,
    )
    .primary()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_debug_prompt_names_the_pod_container_and_image() {
        assert_eq!(
            debug_body("no-shell", "app"),
            "Adds an ephemeral busybox:1.36 container to no-shell that targets app, then opens its shell. Ephemeral containers stay in the pod until it is deleted."
        );
    }
}
