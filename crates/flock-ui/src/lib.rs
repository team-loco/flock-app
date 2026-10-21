pub mod assets;
mod gallery;
pub mod highlight;
pub mod keymap;
pub mod kit;
pub mod prefs;
pub mod screens;
pub mod session;
pub mod settings;
mod shell;
mod store;
pub mod theme;
pub mod updater;

pub use gallery::{Gallery, GalleryPage};
pub use gpui_component::Root;
pub use shell::{NewWindowRequest, Shell};
pub use store::Store;

pub mod actions {
    gpui::actions!(
        flock,
        [
            ToggleSidebar,
            ToggleClusterSwitcher,
            ShowAllClusters,
            RefreshView,
            ToggleCommandPalette,
            FollowLogs,
            CompareAcrossClusters,
            ShowFluxOwner,
            OpenSelectedNewWindow,
            CloseWindow,
            OpenSettings,
            QuickOpen,
            ShowCommands,
            ToggleDock,
            NewTab,
            CloseTab,
            SplitRight,
            OpenSelectedInSplit,
            NextTab,
            PreviousTab,
            GoBack,
            GoForward,
            FocusFilter,
            FocusSidebar,
            FocusContent,
            Quit,
            NewWindow,
            CheckForUpdates,
            RestartToUpdate
        ]
    );

    #[derive(Clone, Copy, Debug, PartialEq, Eq, gpui::Action)]
    #[action(namespace = flock, no_json)]
    pub struct ActivateTab(pub usize);

    #[derive(Clone, Copy, Debug, PartialEq, Eq, gpui::Action)]
    #[action(namespace = flock, no_json)]
    pub struct FocusPane(pub usize);
}

pub fn init(
    forced: Option<theme::Appearance>,
    cx: &mut gpui::App,
) -> Result<(), assets::AssetError> {
    gpui_component::init(cx);
    assets::register_fonts(cx)?;
    theme::init(forced, cx);
    settings::init(cx);
    updater::init(cx);
    kit::init(cx);
    screens::init(cx);
    Ok(())
}
