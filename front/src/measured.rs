//! The DOM backend of ratzilla, drawn as a terminal is: its size as it measured it, and the whole
//! screen again whenever it made its grid again. Three things of ratzilla 0.3:
//!
//! - its own `size` divides the window by a default cell of 10 by 20 pixels whatever the font: the
//!   dashboard was drawn on a part of the page only. `window_size` holds the grid it measured
//! - after any resize, it empties its grid and makes it again when it's next drawn, a size changed
//!   or not, then draws the frame it's given. Of a size changed, the frame of before held cells
//!   beyond the new grid: the page panicked (index out of bounds), frozen. Of a size the same,
//!   ratatui only gives what changed since the last frame: the page stayed black but for the
//!   seconds of the clock
//! - its `clear` does nothing
//!
//! So the screen is kept here, as ratatui last drew it, and given whole when the grid was made
//! again; a frame of the size of before isn't drawn, ratatui draws the next of the new size, whole.

use ratzilla::DomBackend;
use ratzilla::WebEventHandler;
use ratzilla::error::Error;
use ratzilla::event::{KeyEvent, MouseEvent};
use ratzilla::ratatui::backend::{Backend, ClearType, WindowSize};
use ratzilla::ratatui::buffer::{Buffer, Cell};
use ratzilla::ratatui::layout::{Position, Rect, Size};
use ratzilla::web_sys::Element;

/// The id of the grid of the DOM backend, the default one
const GRID: &str = "grid";

pub struct Measured {
    inner: DomBackend,
    size: Size,
    /// The screen as ratatui last drew it, whole
    screen: Buffer,
    /// The grid the screen was last drawn on: another one is a grid made again, empty
    grid: Option<Element>,
}

/// The grid of the DOM backend in the page, None before it's first drawn
fn grid() -> Option<Element> {
    ratzilla::web_sys::window()?
        .document()?
        .get_element_by_id(GRID)
}

fn area(size: Size) -> Rect {
    Rect::new(0, 0, size.width, size.height)
}

impl Measured {
    pub fn new(mut inner: DomBackend) -> std::io::Result<Self> {
        let size = inner.window_size()?.columns_rows;
        Ok(Self {
            inner,
            size,
            screen: Buffer::empty(area(size)),
            grid: None,
        })
    }

    /// Every cell of the screen, on a grid made again
    fn draw_whole(&mut self) -> std::io::Result<()> {
        let screen = &self.screen;
        let cells = screen.content.iter().enumerate().map(|(index, cell)| {
            let (x, y) = screen.pos_of(index);
            (x, y, cell)
        });
        self.inner.draw(cells)
    }
}

impl Backend for Measured {
    type Error = std::io::Error;

    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let changed: Vec<(u16, u16, Cell)> =
            content.map(|(x, y, cell)| (x, y, cell.clone())).collect();
        // nothing drawn: the grid made again after a resize, nothing else
        self.inner.draw(std::iter::empty())?;
        let size = self.inner.window_size()?.columns_rows;
        if size != self.size {
            // a frame of the size of before: ratatui sees the new one at the next, and draws it
            // whole
            self.size = size;
            self.screen = Buffer::empty(area(size));
            self.grid = grid();
            return Ok(());
        }
        let bounds = self.screen.area;
        for (x, y, cell) in &changed {
            if bounds.contains(Position::new(*x, *y)) {
                self.screen[(*x, *y)] = cell.clone();
            }
        }
        let current = grid();
        if current != self.grid {
            // made again, empty: the whole screen, not only what changed
            self.grid = current;
            return self.draw_whole();
        }
        self.inner.draw(
            changed
                .iter()
                .filter(|(x, y, _)| bounds.contains(Position::new(*x, *y)))
                .map(|(x, y, cell)| (*x, *y, cell)),
        )
    }

    fn hide_cursor(&mut self) -> std::io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> std::io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> std::io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> std::io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    /// Every cell blank, which ratzilla doesn't do: ratatui then draws what's not blank only
    fn clear(&mut self) -> std::io::Result<()> {
        self.screen.reset();
        self.draw_whole()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> std::io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> std::io::Result<Size> {
        Ok(self.size)
    }

    fn window_size(&mut self) -> std::io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl WebEventHandler for Measured {
    fn on_mouse_event<F>(&mut self, callback: F) -> Result<(), Error>
    where
        F: FnMut(MouseEvent) + 'static,
    {
        self.inner.on_mouse_event(callback)
    }

    fn clear_mouse_events(&mut self) {
        self.inner.clear_mouse_events();
    }

    fn on_key_event<F>(&mut self, callback: F) -> Result<(), Error>
    where
        F: FnMut(KeyEvent) + 'static,
    {
        self.inner.on_key_event(callback)
    }

    fn clear_key_events(&mut self) {
        self.inner.clear_key_events();
    }
}
