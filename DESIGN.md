# Flock design context

Flock is a native macOS desktop tool for platform engineers monitoring multiple Kubernetes clusters.
Dense tables and document-like resource pages put failures and their causes first.

The maintained visual specification is `design/HANDOFF.md` and `design/screens/*.dc.html`.
`design/v1/*.dc.html` supplies content for pages absent from v2, restyled to the v2 rules.
`docs/decisions.md` records approved deviations. The product name is Flock.

Runtime tokens live in `crates/flock-ui/src/theme.rs`: IBM Plex Sans for UI, IBM Plex Mono for data,
Flock Status for status glyphs; dark and light palettes, spacing, typography, row heights and radii.
Use these tokens directly. No cards or pills; sections use rules, and corners are small.

`docs/ui-kit.md` defines the canonical controls and their behavior. Select owns namespace pickers;
Table owns virtual scrolling, sorting, row selection and keyboard activation. Inspector owns the
right-side details surface. It is non-modal: the table stays usable, and Escape closes event details.
Links to Kubernetes objects emit `Command::OpenResource`.

The shell uses one sidebar and the status-bar cluster switcher. Inspectors animate through the kit's
Presence helpers and honor reduced motion.

Verify changed flows with `flock_shots::Ui` and headless dark/light screenshots. Never open the GUI
for automated verification.

Every pane's page header leads with ghost SVG Back/Forward controls from the kit. Disabled controls
keep their size, tooltips name the destination and shortcut, and right-click opens the active tab's
history menu in nearest-first order. `HeaderNavigation` owns this behavior across shell and screen
headers; the shell routes navigation to the owning pane.

Settings uses the shared Toggle and persisted AppSettings for “Show in menu bar” under Appearance.
The macOS menu-bar item uses native AppKit menus and system typography; its health glyphs follow
Flock's status vocabulary. Its lifecycle and problem click-through follow decision 35.
