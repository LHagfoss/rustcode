use crate::ui::TuiEvent;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{self, Instant};

#[derive(Clone)]
pub(crate) struct FrameRequester {
    requests: mpsc::UnboundedSender<FrameRequest>,
}

pub(crate) struct FrameStream {
    draws: mpsc::UnboundedReceiver<TuiEvent>,
}

struct FrameRequest {
    delay: Duration,
}

fn frame_channel(minimum_interval: Duration) -> (FrameRequester, FrameStream) {
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (draw_tx, draw_rx) = mpsc::unbounded_channel();
    tokio::spawn(run_scheduler(request_rx, draw_tx, minimum_interval));

    (
        FrameRequester {
            requests: request_tx,
        },
        FrameStream { draws: draw_rx },
    )
}

async fn run_scheduler(
    mut requests: mpsc::UnboundedReceiver<FrameRequest>,
    draws: mpsc::UnboundedSender<TuiEvent>,
    minimum_interval: Duration,
) {
    let mut pending: Option<Instant> = None;

    loop {
        match pending {
            Some(deadline) => {
                tokio::select! {
                    request = requests.recv() => {
                        let Some(request) = request else {
                            return;
                        };
                        let requested_deadline = Instant::now()
                            + request.delay.max(minimum_interval);
                        pending = Some(deadline.min(requested_deadline));
                    }
                    _ = time::sleep_until(deadline) => {
                        if draws.send(TuiEvent::Draw).is_err() {
                            return;
                        }
                        pending = None;
                    }
                }
            }
            None => {
                let Some(request) = requests.recv().await else {
                    return;
                };
                pending = Some(Instant::now() + request.delay.max(minimum_interval));
            }
        }
    }
}

impl FrameRequester {
    pub(crate) fn new(minimum_interval: Duration) -> (Self, FrameStream) {
        frame_channel(minimum_interval)
    }

    pub(crate) fn schedule_frame(&self) {
        self.schedule_frame_in(Duration::ZERO);
    }

    pub(crate) fn schedule_frame_in(&self, delay: Duration) {
        let _ = self.requests.send(FrameRequest { delay });
    }
}

impl FrameStream {
    pub(crate) async fn next(&mut self) -> Option<TuiEvent> {
        self.draws.recv().await
    }

    pub(crate) fn try_next(&mut self) -> Option<TuiEvent> {
        self.draws.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameStream, frame_channel};
    use crate::ui::TuiEvent;
    use std::time::Duration;

    async fn next_draw(stream: &mut FrameStream) -> Option<TuiEvent> {
        stream.next().await
    }

    #[tokio::test(start_paused = true)]
    async fn immediate_requests_coalesce_into_one_draw() {
        let (requester, mut stream) = frame_channel(Duration::from_millis(10));
        requester.schedule_frame();
        requester.schedule_frame();

        tokio::task::yield_now().await;
        assert!(stream.try_next().is_none());

        tokio::time::advance(Duration::from_millis(9)).await;
        tokio::task::yield_now().await;
        assert!(stream.try_next().is_none());

        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(next_draw(&mut stream).await, Some(TuiEvent::Draw));
        assert!(stream.try_next().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_requests_do_not_fire_early() {
        let (requester, mut stream) = frame_channel(Duration::from_millis(10));
        requester.schedule_frame_in(Duration::from_millis(20));

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(19)).await;
        tokio::task::yield_now().await;
        assert!(stream.try_next().is_none());

        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(next_draw(&mut stream).await, Some(TuiEvent::Draw));
    }

    #[tokio::test]
    async fn stream_closes_when_all_requesters_are_dropped() {
        let (requester, mut stream) = frame_channel(Duration::from_millis(1));
        drop(requester);

        assert_eq!(next_draw(&mut stream).await, None);
    }
}
