//! The DOM backend of ratzilla, its size as it measured it: its own `size` divides the window by a
//! default cell of 10 by 20 pixels whatever the font, and the dashboard was drawn on a part of the
//! page only. `window_size` holds the grid it measured, read again after each draw, when it
//! measures again after a resize: the next frame takes it

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
        self.inner.draw(content)?;
        self.size = self.inner.window_size()?.columns_rows;
        Ok(())
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
