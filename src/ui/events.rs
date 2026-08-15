use crossterm::event;
use futures_util::StreamExt;
use std::io;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TuiEvent {
    Key(event::KeyEvent),
    Paste(String),
    Resize { width: u16, height: u16 },
    FocusGained,
    FocusLost,
    Draw,
}

pub(crate) struct TuiEventStream {
    stream: Option<event::EventStream>,
    paused: bool,
}

fn normalize_event(event: event::Event) -> Option<TuiEvent> {
    match event {
        event::Event::Key(key) if key.kind == event::KeyEventKind::Release => None,
        event::Event::Key(key) => Some(TuiEvent::Key(key)),
        event::Event::Paste(text) => Some(TuiEvent::Paste(text)),
        event::Event::Resize(width, height) => Some(TuiEvent::Resize { width, height }),
        event::Event::FocusGained => Some(TuiEvent::FocusGained),
        event::Event::FocusLost => Some(TuiEvent::FocusLost),
        event::Event::Mouse(_) => None,
    }
}

impl TuiEventStream {
    pub(crate) fn new() -> Self {
        Self {
            stream: Some(event::EventStream::new()),
            paused: false,
        }
    }

    pub(crate) fn pause(&mut self) {
        self.stream = None;
        self.paused = true;
    }

    pub(crate) fn resume(&mut self) {
        if self.paused {
            self.stream = Some(event::EventStream::new());
            self.paused = false;
        }
    }

    pub(crate) async fn next(&mut self) -> io::Result<Option<TuiEvent>> {
        if self.paused {
            return Ok(None);
        }

        loop {
            let Some(stream) = self.stream.as_mut() else {
                return Ok(None);
            };
            let event = stream.next().await;
            match event {
                Some(Ok(event)) => {
                    if let Some(event) = normalize_event(event) {
                        return Ok(Some(event));
                    }
                }
                Some(Err(error)) => return Err(error),
                None => return Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TuiEvent, normalize_event};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    #[test]
    fn normalizes_key_press_and_ignores_release() {
        let press = KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
        );
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL,
            KeyEventKind::Release,
        );

        assert_eq!(
            normalize_event(Event::Key(press)),
            Some(TuiEvent::Key(press))
        );
        assert_eq!(normalize_event(Event::Key(release)), None);
    }

    #[test]
    fn normalizes_paste_focus_and_resize_events() {
        assert_eq!(
            normalize_event(Event::Paste("hello".to_owned())),
            Some(TuiEvent::Paste("hello".to_owned()))
        );
        assert_eq!(
            normalize_event(Event::FocusGained),
            Some(TuiEvent::FocusGained)
        );
        assert_eq!(normalize_event(Event::FocusLost), Some(TuiEvent::FocusLost));
        assert_eq!(
            normalize_event(Event::Resize(120, 40)),
            Some(TuiEvent::Resize {
                width: 120,
                height: 40
            })
        );
    }
}
