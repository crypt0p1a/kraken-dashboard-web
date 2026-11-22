//! The web page of kraken-dashboard-web: the dashboard of kraken-trading-bot as the terminal shows
//! it, its screen and its logic compiled to WebAssembly, drawn by ratzilla in the page. It reads
//! what the bot pushed to the backend, a screen at a time: `GET /version`, then `GET /data` of the
//! range and the account shown, when the bot pushed or a key changes what's shown.
//!
//! Read only: a tick is asked for from the dashboard of the terminal only.

// the implementation ratatui asks for, linked in
use critical_section as _;
use gloo_net::http::Request;
use kraken_dashboard::app::App;
use kraken_dashboard::data::{Data, Range, Source};
use kraken_dashboard::keys::{KeyCode, KeyEvent, KeyModifiers};
use kraken_dashboard::settings::Settings;
use kraken_dashboard::ui;
use ratzilla::ratatui::Terminal;
use ratzilla::{DomBackend, WebRenderer};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

mod measured;

/// How often the backend is asked whether the bot pushed
const POLL: Duration = Duration::from_secs(2);

/// Where the screen is kept, in the browser
const SETTINGS: &str = "kraken-dashboard";

/// The screens read from the backend, of its version: forgotten when the bot pushes again
#[derive(Default)]
struct Pushed {
    version: Option<i64>,
    screens: HashMap<(Range, Option<i64>), Data>,
}

impl Source for Pushed {
    fn data_version(&self) -> Result<i64, String> {
        self.version
            .ok_or_else(|| "reading the backend…".to_string())
    }

    fn load_range(&self, range: Range, _now: i64, account: Option<i64>) -> Result<Data, String> {
        self.screens
            .get(&(range, account))
            .cloned()
            .ok_or_else(|| "reading the backend…".to_string())
    }

    fn request_tick(&self, _at: i64, _account: Option<i64>) -> Result<(), String> {
        Err("read only on the web: t in the dashboard of the terminal".into())
    }
}

struct State {
    app: App,
    pushed: Pushed,
    /// The settings last kept, written again when they change
    kept: Option<Settings>,
    /// The settings kept, applied again once the first screen is read: the account shown needs the
    /// accounts of the bot, unknown before. Nothing is written meanwhile
    pending: Option<Settings>,
    /// The last error is the poll's: gone as soon as the backend answers again
    poll_failed: bool,
}

type Shared = Rc<RefCell<State>>;

fn main() -> std::io::Result<()> {
    std::panic::set_hook(Box::new(|info| {
        ratzilla::web_sys::console::error_1(&info.to_string().into());
    }));
    let mut app = App::new(origin());
    // the screen as it was left: its range at once, its account once the accounts are known
    let kept = load_settings();
    if let Some(settings) = &kept {
        app.apply(settings.clone());
    }
    let state: Shared = Rc::new(RefCell::new(State {
        app,
        pushed: Pushed::default(),
        pending: kept.clone(),
        kept,
        poll_failed: false,
    }));

    let backend = measured::Measured::new(DomBackend::new().map_err(std::io::Error::other)?)?;
    let terminal = Terminal::new(backend)?;
    listen_to_the_keys(state.clone())?;
    listen_to_the_taps(state.clone())?;

    wasm_bindgen_futures::spawn_local(poll(state.clone()));

    terminal.draw_web(move |frame| {
        let state = state.borrow();
        ui::render(frame, &state.app);
    });
    Ok(())
}

/// The keys of the whole page, as a terminal gets them: ratzilla listens on its grid only, which
/// needs the focus and is made again at each resize, its listener lost
fn listen_to_the_keys(state: Shared) -> std::io::Result<()> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let document = web_sys::window()
        .and_then(|it| it.document())
        .ok_or_else(|| std::io::Error::other("no document"))?;
    let listener =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            // the shortcuts of the browser stay its own: ctrl+1 is the first tab, not the range
            if event.meta_key() || event.alt_key() || event.ctrl_key() {
                return;
            }
            on_key(&state, key_of(event.into()));
        });
    document
        .add_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref())
        .map_err(|_| std::io::Error::other("the keys can't be listened to"))?;
    // for the life of the page
    listener.forget();
    Ok(())
}

/// The taps of the whole page, a key each when the dashboard says one is under it: a tablet or a
/// phone has no keyboard. The cell tapped is read from the grid of ratzilla in the page, made
/// again at each resize
fn listen_to_the_taps(state: Shared) -> std::io::Result<()> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let document = web_sys::window()
        .and_then(|it| it.document())
        .ok_or_else(|| std::io::Error::other("no document"))?;
    let listener =
        Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |event: web_sys::MouseEvent| {
            let Some((area, column, row)) = cell_at(event.client_x(), event.client_y()) else {
                return;
            };
            let key = {
                let state = state.borrow();
                kraken_dashboard::ui::tap(&state.app, area, column, row)
            };
            if let Some(code) = key {
                on_key(&state, KeyEvent::new(code, KeyModifiers::NONE));
            }
        });
    document
        .add_event_listener_with_callback("click", listener.as_ref().unchecked_ref())
        .map_err(|_| std::io::Error::other("the taps can't be listened to"))?;
    // for the life of the page
    listener.forget();
    Ok(())
}

/// The screen and the cell under a point of the page: the grid of ratzilla, a line of cells per
/// `pre`, as many cells as characters
fn cell_at(x: i32, y: i32) -> Option<(ratzilla::ratatui::layout::Rect, u16, u16)> {
    let grid = web_sys::window()?.document()?.get_element_by_id("grid")?;
    let lines = grid.children();
    let rows = lines.length() as u16;
    let columns = lines.item(0)?.text_content()?.chars().count() as u16;
    if rows == 0 || columns == 0 {
        return None;
    }
    let bounds = grid.get_bounding_client_rect();
    let (cell_width, cell_height) = (
        bounds.width() / f64::from(columns),
        bounds.height() / f64::from(rows),
    );
    let column = ((f64::from(x) - bounds.left()) / cell_width).floor();
    let row = ((f64::from(y) - bounds.top()) / cell_height).floor();
    if column < 0.0 || row < 0.0 || column >= f64::from(columns) || row >= f64::from(rows) {
        return None;
    }
    let area = ratzilla::ratatui::layout::Rect::new(0, 0, columns, rows);
    Some((area, column as u16, row as u16))
}

/// Where the page comes from, as the header of the dashboard names its source
fn origin() -> String {
    web_sys::window()
        .and_then(|it| it.location().host().ok())
        .unwrap_or_else(|| "web".to_string())
}

/// A key of the page, as the dashboard knows it: shift and tab is the tab backwards
fn key_of(key: ratzilla::event::KeyEvent) -> KeyEvent {
    use ratzilla::event::KeyCode as Page;
    let code = match key.code {
        Page::Char(char) => KeyCode::Char(char),
        Page::Enter => KeyCode::Enter,
        Page::Esc => KeyCode::Esc,
        Page::Tab if key.shift => KeyCode::BackTab,
        Page::Tab => KeyCode::Tab,
        Page::Left => KeyCode::Left,
        Page::Right => KeyCode::Right,
        Page::Up => KeyCode::Up,
        Page::Down => KeyCode::Down,
        Page::Home => KeyCode::Home,
        Page::End => KeyCode::End,
        Page::PageUp => KeyCode::PageUp,
        Page::PageDown => KeyCode::PageDown,
        _ => KeyCode::Other,
    };
    let modifiers = if key.ctrl {
        KeyModifiers::CONTROL
    } else {
        KeyModifiers::NONE
    };
    KeyEvent::new(code, modifiers)
}

fn on_key(state: &Shared, key: KeyEvent) {
    let changed = {
        let mut state = state.borrow_mut();
        let State { app, pushed, .. } = &mut *state;
        let changed = app.on_key(key);
        // a page isn't quit: q does nothing
        app.quit = false;
        app.ask_tick(pushed);
        changed
    };
    keep_settings(state);
    if changed {
        // what's shown changed: read at once when it's there, from the backend else
        wasm_bindgen_futures::spawn_local(show(state.clone(), true));
    }
}

/// Asks the backend whether the bot pushed, again and again; the screen shown read again when it
/// did, or when it's still missing, its last read failed
async fn poll(state: Shared) {
    loop {
        match fetch_version().await {
            Ok(version) => {
                let missing = {
                    let mut state = state.borrow_mut();
                    if state.pushed.version != Some(version) {
                        state.pushed.version = Some(version);
                        state.pushed.screens.clear();
                    }
                    if std::mem::take(&mut state.poll_failed) {
                        state.app.error = None;
                    }
                    let shown = (state.app.range, state.app.view);
                    !state.pushed.screens.contains_key(&shown)
                };
                if missing {
                    show(state.clone(), false).await;
                }
            }
            Err(error) => {
                let mut state = state.borrow_mut();
                state.app.error = Some(error);
                state.poll_failed = true;
            }
        }
        gloo_timers::future::sleep(POLL).await;
    }
}

/// The screen of what's shown, read from the backend unless it was already
async fn show(state: Shared, force: bool) {
    let (range, account, known, version) = {
        let state = state.borrow();
        let key = (state.app.range, state.app.view);
        (
            key.0,
            key.1,
            state.pushed.screens.contains_key(&key),
            state.pushed.version,
        )
    };
    if !known {
        let read = fetch_screen(range, account).await;
        let mut state = state.borrow_mut();
        // the bot pushed meanwhile: this screen is of before, the read of the new one follows
        if state.pushed.version != version {
            return;
        }
        match read {
            Ok(data) => {
                state.pushed.screens.insert((range, account), data);
            }
            Err(error) => {
                state.app.error = Some(error);
                return;
            }
        }
    }
    let moved = {
        let mut state = state.borrow_mut();
        let State {
            app,
            pushed,
            pending,
            ..
        } = &mut *state;
        // a key changed it meanwhile: its own read follows
        if (app.range, app.view) != (range, account) {
            return;
        }
        app.refresh(pushed, force || !known);
        // the first screen read: the accounts known, the screen as it was left
        match pending.take() {
            Some(settings) if app.error.is_none() => {
                app.apply(settings);
                (app.range, app.view) != (range, account)
            }
            other => {
                *pending = other;
                false
            }
        }
    };
    if moved {
        Box::pin(show(state, true)).await;
    }
}

/// The session over (401): the page to log in
fn logged_out(status: u16) {
    if status == 401
        && let Some(window) = web_sys::window()
    {
        let _ = window.location().set_href("/login");
    }
}

async fn fetch_version() -> Result<i64, String> {
    #[derive(serde::Deserialize)]
    struct Version {
        version: u64,
    }
    let response = Request::get("/version")
        .send()
        .await
        .map_err(|error| format!("backend not reached: {error}"))?;
    if !response.ok() {
        logged_out(response.status());
        return Err(format!("backend: {}", response.status()));
    }
    let version: Version = response
        .json()
        .await
        .map_err(|error| format!("backend: {error}"))?;
    Ok(version.version as i64)
}

/// A screen as the bot pushed it; an account the bot doesn't show apart anymore: all of them
async fn fetch_screen(range: Range, account: Option<i64>) -> Result<Data, String> {
    let name = serde_json::to_value(range)
        .ok()
        .and_then(|it| it.as_str().map(str::to_string))
        .unwrap_or_default();
    let url = |account: Option<i64>| match account {
        Some(account) => format!("/data?range={name}&account={account}"),
        None => format!("/data?range={name}"),
    };
    let mut response = Request::get(&url(account))
        .send()
        .await
        .map_err(|error| format!("backend not reached: {error}"))?;
    if response.status() == 404 && account.is_some() {
        response = Request::get(&url(None))
            .send()
            .await
            .map_err(|error| format!("backend not reached: {error}"))?;
    }
    if !response.ok() {
        logged_out(response.status());
        let text = response.text().await.unwrap_or_default();
        return Err(format!("backend: {text}"));
    }
    response
        .json()
        .await
        .map_err(|error| format!("backend: {error}"))
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn load_settings() -> Option<Settings> {
    let text = storage()?.get_item(SETTINGS).ok().flatten()?;
    serde_json::from_str(&text).ok()
}

/// Written as soon as they change: the dashboard works the same without them. Not before the kept
/// ones are applied, which would lose their account
fn keep_settings(state: &Shared) {
    let mut state = state.borrow_mut();
    if state.pending.is_some() {
        return;
    }
    let current = state.app.settings();
    if state.kept.as_ref() == Some(&current) {
        return;
    }
    if let (Some(storage), Ok(json)) = (storage(), serde_json::to_string(&current)) {
        let _ = storage.set_item(SETTINGS, &json);
    }
    state.kept = Some(current);
}
