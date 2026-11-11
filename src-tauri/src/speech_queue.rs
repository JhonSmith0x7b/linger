//! Ordered playback with one segment of bounded synthesis lookahead.
use std::future::Future;
use tokio::sync::mpsc;

pub async fn run<T, Generate, GFut, Play, PFut>(
    parts: Vec<String>,
    generate: Generate,
    play: Play,
) -> Result<u64, String>
where
    Generate: Fn(String) -> GFut,
    GFut: Future<Output = Result<(T, u64), String>>,
    Play: Fn(T) -> PFut,
    PFut: Future<Output = Result<(), String>>,
{
    let (sender, mut receiver) = mpsc::channel(1);
    let producer = async {
        for part in parts {
            // Reserve before synthesis: a ready segment also consumes the lookahead
            // allowance, so a slow player cannot accumulate extra audio requests.
            let permit = sender.reserve().await.map_err(|_| "语音播放队列已关闭")?;
            let result = generate(part).await;
            let failed = result.is_err();
            permit.send(result);
            // Deliver failures in order, after any audio already being played.
            if failed {
                break;
            }
        }
        drop(sender);
        Ok::<(), String>(())
    };
    let consumer = async {
        let mut generation_ms: u64 = 0;
        while let Some(result) = receiver.recv().await {
            let (audio, elapsed) = result?;
            generation_ms = generation_ms.saturating_add(elapsed);
            play(audio).await?;
        }
        Ok::<u64, String>(generation_ms)
    };
    // No spawned producer can outlive the turn. Dropping this future, or a
    // playback error, drops the in-flight generation and all queued audio.
    let (_, generation_ms) = tokio::try_join!(producer, consumer)?;
    Ok(generation_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    };
    use tokio::sync::Notify;

    fn parts() -> Vec<String> {
        ["one", "two", "three"].map(String::from).into()
    }

    #[tokio::test]
    async fn overlaps_playback_but_prepares_only_one_segment_ahead() {
        let generated = AtomicUsize::new(0);
        let second_ready = Notify::new();
        let played = Mutex::new(Vec::new());
        let (generated, second_ready, played) = (&generated, &second_ready, &played);
        let elapsed = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run(
                parts(),
                |part| async move {
                    let count = generated.fetch_add(1, Ordering::SeqCst) + 1;
                    if count == 2 {
                        second_ready.notify_one();
                    }
                    Ok((part, 10))
                },
                |part| async move {
                    if part == "one" {
                        // This would deadlock with sequential generation/playback.
                        second_ready.notified().await;
                        tokio::task::yield_now().await;
                        assert_eq!(generated.load(Ordering::SeqCst), 2);
                    }
                    played.lock().unwrap().push(part);
                    Ok(())
                },
            ),
        )
        .await
        .expect("pipeline did not overlap")
        .unwrap();
        assert_eq!(elapsed, 30);
        assert_eq!(*played.lock().unwrap(), parts());
    }

    #[tokio::test]
    async fn generation_failure_waits_for_current_audio_and_stops_later_requests() {
        let failed = Notify::new();
        let finished_playing = AtomicBool::new(false);
        let requests = AtomicUsize::new(0);
        let (failed, finished_playing, requests) = (&failed, &finished_playing, &requests);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run(
                parts(),
                |part| async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    if part == "two" {
                        failed.notify_one();
                        Err("synthesis failed".into())
                    } else {
                        Ok((part, 1))
                    }
                },
                |part| async move {
                    assert_eq!(part, "one");
                    failed.notified().await;
                    finished_playing.store(true, Ordering::SeqCst);
                    Ok(())
                },
            ),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "synthesis failed");
        assert!(finished_playing.load(Ordering::SeqCst));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    struct Dropped<'a>(&'a AtomicBool);
    impl Drop for Dropped<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn playback_failure_drops_in_flight_generation() {
        let started = Notify::new();
        let dropped = AtomicBool::new(false);
        let (started, dropped) = (&started, &dropped);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run(
                parts(),
                |part| async move {
                    if part == "two" {
                        let _guard = Dropped(dropped);
                        started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Ok((part, 1))
                },
                |_| async {
                    started.notified().await;
                    Err("playback failed".into())
                },
            ),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "playback failed");
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn dropping_turn_cancels_generation_without_detached_work() {
        let started = Notify::new();
        let dropped = AtomicBool::new(false);
        let (started, dropped) = (&started, &dropped);
        let pipeline = run(
            parts(),
            |_| async {
                let _guard = Dropped(dropped);
                started.notify_one();
                std::future::pending::<()>().await;
                Ok(((), 1))
            },
            |_| async { panic!("cancelled audio must not play") },
        );
        tokio::select! {
            _ = pipeline => panic!("generation should still be pending"),
            _ = started.notified() => {}
        }
        assert!(dropped.load(Ordering::SeqCst));
    }
}
