use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use flock::{Preset, Screen, WINDOW_HEIGHT, WINDOW_WIDTH, bind_keys, root_view};
use flock_core::app_settings::Highlight;
use flock_core::history::Location;
use flock_core::tabs::{SavedPane, SavedWindow};
use flock_core::{AppState, Backend, Command, Receiver, Sender, Update};
use flock_demo::DemoBackend;
use flock_ui::theme::Appearance;
use flock_ui::{NewWindowRequest, Root, Shell, Store, assets::Assets};
use gpui::{
    AnyView, AnyWindowHandle, App, AppContext, Bounds, Entity, HeadlessAppContext, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput,
    PlatformTextSystem, Point, ScrollDelta, ScrollWheelEvent, Size, TestAppContext, TestDispatcher,
    VisualTestContext, Window, px, size,
};
use image::RgbaImage;

mod fixture;

pub use fixture::StateFixture;

const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(300);
const POLL: Duration = Duration::from_millis(5);
const SEED: u64 = 0;
const SPACE: &str = "space";
const SINGLE: usize = 1;
const DRAG_MOVES: u16 = 10;
const DOUBLE: usize = 2;

fn text_system() -> Arc<dyn PlatformTextSystem> {
    Arc::new(gpui_apple::AppleTextSystem::new())
}

fn window_size() -> Size<Pixels> {
    size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Highlights {
    pub primary: Highlight,
    pub secondary: Highlight,
}

fn setup(appearance: Appearance, highlights: Highlights, cx: &mut App) -> anyhow::Result<()> {
    flock_ui::init(Some(appearance), cx)?;
    bind_keys(cx);
    cx.set_reduce_motion(true);
    flock_ui::settings::update(cx, |settings| {
        settings.primary = highlights.primary;
        settings.secondary = highlights.secondary;
    })
    .map_err(|error| anyhow!("applying highlights: {error}"))
}

pub type BackendFactory = Rc<dyn Fn() -> anyhow::Result<Box<dyn Backend>>>;

#[derive(Clone)]
pub enum Source {
    Demo,
    Fixture(StateFixture),
    Backend(BackendFactory),
}

type Feed = (Receiver<Update>, Sender<Command>);

fn scripted() -> anyhow::Result<(Feed, Sender<Update>)> {
    let (update_tx, update_rx) = async_channel::unbounded();
    let (command_tx, command_rx) = async_channel::unbounded::<Command>();
    std::thread::spawn(move || while command_rx.recv_blocking().is_ok() {});
    let first = Update::Snapshot(Box::new(flock_demo::snapshot()));
    update_tx
        .try_send(first)
        .map_err(|error| anyhow!("scripting the first snapshot: {error}"))?;
    Ok(((update_rx, command_tx), update_tx))
}

fn start(backend: Box<dyn Backend>) -> Feed {
    let (update_tx, update_rx) = async_channel::unbounded();
    let (command_tx, command_rx) = async_channel::unbounded();
    backend.start(update_tx, command_rx);
    (update_rx, command_tx)
}

fn channels(source: &Source) -> anyhow::Result<Feed> {
    match source {
        Source::Demo => Ok(start(Box::new(DemoBackend))),
        Source::Fixture(state) => Ok(fixture::backend(*state)),
        Source::Backend(make) => Ok(start(make()?)),
    }
}

fn sample_tabs(screen: Screen, preset: &Preset, source: &Source) -> Option<SavedWindow> {
    match source {
        Source::Demo => screen.tabs(preset),
        Source::Fixture(_) => screen.tabs(preset),
        Source::Backend(_) => None,
    }
}

fn view(
    screen: Screen,
    preset: &Preset,
    sample_tabs: Option<SavedWindow>,
    (updates, commands): Feed,
    window: &mut Window,
    cx: &mut App,
) -> (AnyView, Option<Entity<Shell>>) {
    let store = cx.new(|cx| Store::new(updates, commands, cx));
    let view = root_view(screen, preset, store, window, cx);
    let shell = view.clone().downcast::<Shell>().ok();
    if let Some((shell, tabs)) = shell.as_ref().zip(sample_tabs) {
        shell.update(cx, |shell, cx| shell.restore_tabs(&tabs, window, cx));
    }
    (view, shell)
}

fn settle<C: AppContext>(
    cx: &C,
    park: impl Fn(),
    shell: Option<&Entity<Shell>>,
) -> anyhow::Result<()> {
    let start = Instant::now();
    loop {
        park();
        let settled = shell.is_none_or(|shell| cx.read_entity(shell, |shell, _| shell.settled()));
        if settled {
            break;
        }
        if start.elapsed() > SETTLE_TIMEOUT {
            bail!("the demo backend did not settle within {SETTLE_TIMEOUT:?}");
        }
        std::thread::sleep(POLL);
    }
    let quiet = Instant::now();
    while quiet.elapsed() < QUIET {
        park();
        std::thread::sleep(POLL);
    }
    park();
    Ok(())
}

#[derive(Clone, Debug)]
enum Step {
    Keys(String),
    Text(String),
    Click {
        at: Point<Pixels>,
        button: MouseButton,
        count: usize,
        modifiers: Modifiers,
    },
    Hover(Point<Pixels>),
    Scroll(Point<Pixels>, Point<Pixels>),
    Drag {
        from: Point<Pixels>,
        to: Point<Pixels>,
    },
}

fn mouse_down(
    position: Point<Pixels>,
    button: MouseButton,
    count: usize,
    modifiers: Modifiers,
) -> PlatformInput {
    PlatformInput::MouseDown(MouseDownEvent {
        button,
        position,
        modifiers,
        click_count: count,
        first_mouse: false,
    })
}

fn mouse_up(
    position: Point<Pixels>,
    button: MouseButton,
    count: usize,
    modifiers: Modifiers,
) -> PlatformInput {
    PlatformInput::MouseUp(MouseUpEvent {
        button,
        position,
        modifiers,
        click_count: count,
    })
}

fn mouse_move(position: Point<Pixels>, pressed: Option<MouseButton>) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position,
        pressed_button: pressed,
        modifiers: Modifiers::none(),
    })
}

fn parse(keystroke: &str) -> anyhow::Result<Keystroke> {
    Keystroke::parse(keystroke).map_err(|error| anyhow!("keystroke {keystroke:?}: {error}"))
}

fn apply(step: &Step, window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
    match step {
        Step::Keys(keys) => {
            for key in keys.split_whitespace() {
                window.dispatch_keystroke(parse(key)?, cx);
            }
        }
        Step::Text(text) => {
            for char in text.chars() {
                let key = char.to_string();
                let key = if char == ' ' { SPACE.to_owned() } else { key };
                window.dispatch_keystroke(parse(&key)?, cx);
            }
        }
        Step::Click {
            at,
            button,
            count,
            modifiers,
        } => {
            for count in 1..=*count {
                window.dispatch_event(mouse_down(*at, *button, count, *modifiers), cx);
                window.dispatch_event(mouse_up(*at, *button, count, *modifiers), cx);
            }
        }
        Step::Scroll(position, delta) => {
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: *position,
                    delta: ScrollDelta::Pixels(*delta),
                    ..ScrollWheelEvent::default()
                }),
                cx,
            );
        }
        Step::Hover(position) => {
            window.dispatch_event(mouse_move(*position, None), cx);
        }
        Step::Drag { from, to } => {
            window.dispatch_event(mouse_move(*from, None), cx);
            let none = Modifiers::none();
            window.dispatch_event(mouse_down(*from, MouseButton::Left, SINGLE, none), cx);
            for step in 1..=DRAG_MOVES {
                let along = f32::from(step) / f32::from(DRAG_MOVES);
                let at = *from + (*to - *from) * along;
                window.dispatch_event(mouse_move(at, Some(MouseButton::Left)), cx);
            }
            window.dispatch_event(mouse_up(*to, MouseButton::Left, SINGLE, none), cx);
        }
    }
    Ok(())
}

struct Headless {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    shell: Option<Entity<Shell>>,
}

impl Headless {
    fn open(
        screen: Screen,
        preset: &Preset,
        appearance: Appearance,
        highlights: Highlights,
        source: &Source,
    ) -> anyhow::Result<Self> {
        let backend = channels(source)?;
        let tabs = sample_tabs(screen, preset, source);
        let mut cx = HeadlessAppContext::with_platform(
            text_system(),
            Arc::new(Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| setup(appearance, highlights, cx))?;
        let mut shell = None;
        let window = cx.open_window(window_size(), |window, cx| {
            let (view, built) = view(screen, preset, tabs, backend, window, cx);
            shell = built;
            cx.new(|cx| Root::new(view, window, cx))
        })?;
        let mut headless = Self {
            cx,
            window: window.into(),
            shell,
        };
        headless.settle()?;
        Ok(headless)
    }

    fn settle(&mut self) -> anyhow::Result<()> {
        let dispatcher = self.cx.background_executor().clone();
        settle(
            &self.cx,
            || dispatcher.run_until_parked(),
            self.shell.as_ref(),
        )
    }

    fn run(&mut self, step: &Step) -> anyhow::Result<()> {
        self.cx
            .update_window(self.window, |_, window, cx| apply(step, window, cx))??;
        self.settle()
    }

    fn image(&mut self) -> anyhow::Result<RgbaImage> {
        self.cx.capture_screenshot(self.window)
    }

    fn restore(&mut self, saved: &SavedWindow) -> anyhow::Result<()> {
        let shell = self
            .shell
            .clone()
            .ok_or_else(|| anyhow!("the screen has no shell to restore tabs into"))?;
        self.cx.update_window(self.window, |_, window, cx| {
            shell.update(cx, |shell, cx| shell.restore_tabs(saved, window, cx));
        })?;
        self.settle()
    }

    fn wait_until(
        &mut self,
        timeout: Duration,
        done: &dyn Fn(&AppState) -> bool,
    ) -> anyhow::Result<()> {
        let shell = self
            .shell
            .clone()
            .ok_or_else(|| anyhow!("the screen has no shell to wait on"))?;
        let start = Instant::now();
        loop {
            self.settle()?;
            if self.cx.read_entity(&shell, |shell, _| done(shell.state())) {
                return Ok(());
            }
            if start.elapsed() > timeout {
                bail!("the condition did not hold within {timeout:?}");
            }
        }
    }
}

pub fn render(
    screen: Screen,
    preset: &Preset,
    appearance: Appearance,
) -> anyhow::Result<RgbaImage> {
    Headless::open(
        screen,
        preset,
        appearance,
        Highlights::default(),
        &Source::Demo,
    )?
    .image()
}

pub fn render_state(
    screen: Screen,
    preset: &Preset,
    appearance: Appearance,
    highlights: Highlights,
    state: Option<StateFixture>,
) -> anyhow::Result<RgbaImage> {
    let source = state.map_or(Source::Demo, Source::Fixture);
    Headless::open(screen, preset, appearance, highlights, &source)?.image()
}

pub fn one_tab(location: Location) -> SavedWindow {
    SavedWindow {
        panes: vec![SavedPane {
            tabs: vec![location],
            active: 0,
        }],
        ..SavedWindow::default()
    }
}

pub struct LiveShots<'a> {
    pub appearance: Appearance,
    pub highlights: Highlights,
    pub timeout: Duration,
    pub ready: &'a dyn Fn(&AppState, &Location) -> bool,
}

pub fn render_live(
    make: &BackendFactory,
    shots: &LiveShots<'_>,
    locations: &[Location],
    mut save: impl FnMut(&Location, RgbaImage) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let source = Source::Backend(make.clone());
    let mut headless = Headless::open(
        Screen::Flock,
        &Preset::default(),
        shots.appearance,
        shots.highlights,
        &source,
    )?;
    for location in locations {
        headless.restore(&one_tab(location.clone()))?;
        headless
            .wait_until(shots.timeout, &|state| (shots.ready)(state, location))
            .with_context(|| format!("waiting for {location:?}"))?;
        save(location, headless.image()?)?;
    }
    Ok(())
}

pub fn save(image: &RgbaImage, path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    image
        .save(path)
        .with_context(|| format!("writing {}", path.display()))
}

pub struct Ui {
    cx: VisualTestContext,
    new_windows: Receiver<Vec<Command>>,
    shell: Entity<Shell>,
    screen: Screen,
    preset: Preset,
    appearance: Appearance,
    source: Source,
    steps: Vec<Step>,
}

impl Ui {
    pub fn open(screen: Screen) -> anyhow::Result<Self> {
        Self::with(screen, Preset::default(), Appearance::Dark)
    }

    pub fn with(screen: Screen, preset: Preset, appearance: Appearance) -> anyhow::Result<Self> {
        Self::build(screen, preset, appearance, Source::Demo)
    }

    pub fn with_state(screen: Screen, state: StateFixture) -> anyhow::Result<Self> {
        Self::build(
            screen,
            Preset::default(),
            Appearance::Dark,
            Source::Fixture(state),
        )
    }

    pub fn live(screen: Screen, preset: Preset, make: BackendFactory) -> anyhow::Result<Self> {
        Self::build(screen, preset, Appearance::Dark, Source::Backend(make))
    }

    pub fn scripted(screen: Screen) -> anyhow::Result<(Self, Sender<Update>)> {
        let (feed, updates) = scripted()?;
        let ui = Self::build_from(
            screen,
            Preset::default(),
            Appearance::Dark,
            Source::Demo,
            feed,
        )?;
        Ok((ui, updates))
    }

    fn build(
        screen: Screen,
        preset: Preset,
        appearance: Appearance,
        source: Source,
    ) -> anyhow::Result<Self> {
        let feed = channels(&source)?;
        Self::build_from(screen, preset, appearance, source, feed)
    }

    fn build_from(
        screen: Screen,
        preset: Preset,
        appearance: Appearance,
        source: Source,
        backend: Feed,
    ) -> anyhow::Result<Self> {
        let tabs = sample_tabs(screen, &preset, &source);
        let mut app =
            TestAppContext::build_with_text_system(TestDispatcher::new(SEED), None, text_system());
        app.update(|cx| setup(appearance, Highlights::default(), cx))?;
        let mut shell = None;
        let window = app.open_window(window_size(), |window, cx| {
            let (view, built) = view(screen, &preset, tabs, backend, window, cx);
            shell = built;
            Root::new(view, window, cx)
        });
        let shell = shell.ok_or_else(|| anyhow!("{screen:?} is not a shell screen"))?;
        let (opened, new_windows) = async_channel::unbounded();
        app.update(|cx| {
            cx.subscribe(&shell, move |_, request: &NewWindowRequest, _| {
                let _ = opened.try_send(request.0.clone());
            })
            .detach();
        });
        let mut ui = Self {
            new_windows,
            cx: VisualTestContext::from_window(window.into(), &app),
            shell,
            screen,
            preset,
            appearance,
            source,
            steps: Vec::new(),
        };
        ui.settle()?;
        Ok(ui)
    }

    pub fn take_new_window(&self) -> Option<Vec<Command>> {
        self.new_windows.try_recv().ok()
    }

    pub fn state(&self) -> AppState {
        self.cx
            .read_entity(&self.shell, |shell, _| shell.state().clone())
    }

    pub fn read<R>(&self, read: impl FnOnce(&Shell, &App) -> R) -> R {
        self.cx.read_entity(&self.shell, read)
    }

    pub fn restore(&mut self, tabs: &SavedWindow) -> anyhow::Result<()> {
        let shell = self.shell.clone();
        self.cx.update(|window, cx| {
            shell.update(cx, |shell, cx| shell.restore_tabs(tabs, window, cx));
        });
        self.settle()
    }

    pub fn bounds(&mut self, selector: &'static str) -> Option<Bounds<Pixels>> {
        self.cx.debug_bounds(selector)
    }

    pub fn press(&mut self, keys: &str) -> anyhow::Result<()> {
        self.run(Step::Keys(keys.to_owned()))
    }

    pub fn press_unsettled(&mut self, keys: &str) -> anyhow::Result<()> {
        let step = Step::Keys(keys.to_owned());
        self.cx.update(|window, cx| apply(&step, window, cx))?;
        self.steps.push(step);
        self.cx.background_executor.run_until_parked();
        Ok(())
    }

    pub fn type_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.run(Step::Text(text.to_owned()))
    }

    pub fn read_app<R>(&self, read: impl FnOnce(&App) -> R) -> R {
        self.cx.read(read)
    }

    pub fn update_app<R>(&mut self, update: impl FnOnce(&mut App) -> R) -> R {
        self.cx.update(|_, cx| update(cx))
    }

    pub fn dispatch(&mut self, action: Box<dyn gpui::Action>) -> anyhow::Result<()> {
        self.cx
            .update(|window, cx| window.dispatch_action(action, cx));
        self.settle()
    }

    fn center(&mut self, selector: &'static str) -> anyhow::Result<Point<Pixels>> {
        self.bounds(selector)
            .map(|bounds| bounds.center())
            .ok_or_else(|| anyhow!("no element with debug selector {selector:?} was painted"))
    }

    pub fn click(&mut self, selector: &'static str) -> anyhow::Result<()> {
        self.click_with(selector, MouseButton::Left, SINGLE, Modifiers::none())
    }

    pub fn double_click(&mut self, selector: &'static str) -> anyhow::Result<()> {
        self.click_with(selector, MouseButton::Left, DOUBLE, Modifiers::none())
    }

    pub fn right_click(&mut self, selector: &'static str) -> anyhow::Result<()> {
        self.click_with(selector, MouseButton::Right, SINGLE, Modifiers::none())
    }

    pub fn middle_click(&mut self, selector: &'static str) -> anyhow::Result<()> {
        self.click_with(selector, MouseButton::Middle, SINGLE, Modifiers::none())
    }

    pub fn navigate(
        &mut self,
        selector: &'static str,
        direction: gpui::NavigationDirection,
    ) -> anyhow::Result<()> {
        self.click_with(
            selector,
            MouseButton::Navigate(direction),
            SINGLE,
            Modifiers::none(),
        )
    }

    pub fn shift_click(&mut self, selector: &'static str) -> anyhow::Result<()> {
        self.click_with(selector, MouseButton::Left, SINGLE, Modifiers::shift())
    }

    pub fn cmd_click(&mut self, selector: &'static str) -> anyhow::Result<()> {
        self.click_with(selector, MouseButton::Left, SINGLE, Modifiers::command())
    }

    fn click_with(
        &mut self,
        selector: &'static str,
        button: MouseButton,
        count: usize,
        modifiers: Modifiers,
    ) -> anyhow::Result<()> {
        let at = self.center(selector)?;
        self.run(Step::Click {
            at,
            button,
            count,
            modifiers,
        })
    }

    pub fn hover(&mut self, selector: &'static str) -> anyhow::Result<()> {
        let at = self.center(selector)?;
        self.run(Step::Hover(at))
    }

    pub fn drag(&mut self, selector: &'static str, by: Point<Pixels>) -> anyhow::Result<()> {
        let from = self.center(selector)?;
        self.run(Step::Drag {
            from,
            to: from + by,
        })
    }

    pub fn drag_between(&mut self, from: Point<Pixels>, to: Point<Pixels>) -> anyhow::Result<()> {
        self.run(Step::Drag { from, to })
    }

    pub fn click_at(&mut self, at: Point<Pixels>, count: usize) -> anyhow::Result<()> {
        self.run(Step::Click {
            at,
            button: MouseButton::Left,
            count,
            modifiers: Modifiers::none(),
        })
    }

    pub fn clipboard(&self) -> Option<String> {
        self.cx.read_from_clipboard().and_then(|item| item.text())
    }

    pub fn scroll(&mut self, selector: &'static str, delta: Point<Pixels>) -> anyhow::Result<()> {
        let bounds = self
            .bounds(selector)
            .ok_or_else(|| anyhow!("no element {selector:?}"))?;
        self.run(Step::Scroll(bounds.center(), delta))
    }

    pub fn save_png(&self, path: &Path) -> anyhow::Result<()> {
        let mut headless = Headless::open(
            self.screen,
            &self.preset,
            self.appearance,
            Highlights::default(),
            &self.source,
        )?;
        for step in &self.steps {
            headless.run(step)?;
        }
        save(&headless.image()?, path)
    }

    fn run(&mut self, step: Step) -> anyhow::Result<()> {
        self.cx.update(|window, cx| apply(&step, window, cx))?;
        self.steps.push(step);
        self.settle()
    }

    pub fn advance_clock(&mut self, by: Duration) -> anyhow::Result<()> {
        self.cx.background_executor.advance_clock(by);
        std::thread::sleep(by);
        self.cx.update(|window, _| window.refresh());
        self.settle()
    }

    pub fn wait_until(
        &mut self,
        timeout: Duration,
        done: impl Fn(&AppState) -> bool,
    ) -> anyhow::Result<()> {
        let start = Instant::now();
        loop {
            self.settle()?;
            if done(&self.state()) {
                return Ok(());
            }
            if start.elapsed() > timeout {
                bail!("the condition did not hold within {timeout:?}");
            }
        }
    }

    pub fn pause(&mut self, duration: Duration) -> anyhow::Result<()> {
        let start = Instant::now();
        while start.elapsed() < duration {
            self.settle()?;
        }
        Ok(())
    }

    pub fn settle(&mut self) -> anyhow::Result<()> {
        let dispatcher = self.cx.background_executor.clone();
        settle(
            &self.cx,
            || dispatcher.run_until_parked(),
            Some(&self.shell),
        )
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        self.cx.quit();
    }
}
