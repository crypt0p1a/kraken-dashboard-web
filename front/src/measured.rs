//! The DOM backend of ratzilla, its size as it measured it, and its resizes without a frame of
//! before. Two things of ratzilla 0.3:
//!
//! - its own `size` divides the window by a default cell of 10 by 20 pixels whatever the font: the
//!   dashboard was drawn on a part of the page only. `window_size` holds the grid it measured
//! - after a resize, it measures again and makes its grid again when it's next drawn, then draws
//!   the frame it's given, of the size of before: a cell beyond the new grid, and the page panicked
//!   (index out of bounds), frozen. So it's first drawn with nothing, which makes its grid again;
//!   a size changed, the frame of before isn't drawn, the next one is, of the new size

use ratzilla::DomBackend;
use ratzilla::WebEventHandler;
use ratzilla::error::Error;
use ratzilla::event::{KeyEvent, MouseEvent};
use ratzilla::ratatui::backend::{Backend, ClearType, WindowSize};
use ratzilla::ratatui::buffer::Cell;
use ratzilla::ratatui::layout::{Position, Size};

pub struct Measured {
    inner: DomBackend,
    size: Size,
}

impl Measured {
    pub fn new(mut inner: DomBackend) -> std::io::Result<Self> {
        let size = inner.window_size()?.columns_rows;
        Ok(Self { inner, size })
    }
}

impl Backend for Measured {
    type Error = std::io::Error;

    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        // nothing drawn: the grid made again after a resize, nothing else
        self.inner.draw(std::iter::empty())?;
        let size = self.inner.window_size()?.columns_rows;
        if size != self.size {
            // a frame of the size of before: ratatui sees the new one at the next, and draws it whole
            self.size = size;
            return Ok(());
        }
        self.inner.draw(content)
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

    fn clear(&mut self) -> std::io::Result<()> {
        self.inner.clear()
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
