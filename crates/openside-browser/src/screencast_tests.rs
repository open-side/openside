use super::*;
use axum::{response::Html, routing::get, Router};
use std::time::Duration;

fn frame(ack: i64) -> ScreencastFrame {
    ScreencastFrame {
        data: String::new(),
        viewport: (1440.0, 900.0),
        ack,
    }
}

#[test]
fn animated_cdp_producer_cannot_recycle_replacement_credits_between_preview_ticks() {
    for startup in 1..=3 {
        let slot = Slot::default();
        *lock(&slot.policy) = AckPolicy::Paced {
            deferred: [None; 3],
        };
        let mut credits = startup;
        let mut captures = 0;
        let mut acks = 0;
        for millisecond in 0..=3000 {
            if credits > 0 {
                credits -= 1;
                captures += 1;
                assert!(slot.replace(frame(captures)).is_none());
            }
            if millisecond > 0 && millisecond % 500 == 0 {
                let next = lock(&slot.frame).take().unwrap();
                slot.acknowledge(next.ack);
                credits += 1;
                acks += 1;
            }
            assert!(
                captures <= startup + acks,
                "replacement ACK escaped consumer pacing"
            );
        }
        assert_eq!(acks, 6);
        assert_eq!(captures, startup + 5);
    }
    let eager = Slot::default();
    assert_eq!(eager.replace(frame(1)), None);
    assert_eq!(eager.replace(frame(2)), Some(1));
}

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn animated_chromium_preview_captures_only_when_one_paced_ack_releases_a_credit() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(executable) => executable,
        Err(reason) => {
            eprintln!("SKIP animated screencast ACK test: {reason}");
            return;
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", get(|| async {
            Html("<!doctype html><script>let n=0;setInterval(()=>document.body.style.backgroundColor=`rgb(${++n%255},0,0)`,16)</script><body>Animated CDP fixture")
        }))).await.unwrap();
    }));
    let (browser, _) = Browser::launch(crate::BrowserConfig::new(executable))
        .await
        .unwrap();
    let tab = browser
        .create_agent_tab("ack-test", &origin)
        .await
        .unwrap()
        .id;
    let preview = browser
        .screencast_scaled("ack-test", &tab, 40, (720, 450))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while preview.slot.received.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let initial = preview.slot.received.load(Ordering::Relaxed);
    assert!(
        (1..=3).contains(&initial),
        "startup captured {initial} frames without ACKs"
    );
    assert_eq!(preview.slot.acknowledgements.load(Ordering::Relaxed), 0);
    for tick in 1..=3 {
        let frame = preview.next().await;
        preview.ack(&frame);
        tokio::time::timeout(Duration::from_secs(3), async {
            while preview.slot.received.load(Ordering::Relaxed) < initial + tick {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            preview.slot.received.load(Ordering::Relaxed),
            initial + tick
        );
        assert_eq!(preview.slot.acknowledgements.load(Ordering::Relaxed), tick);
    }
    preview.stop();
    let takeover = browser.screencast("ack-test", &tab, 60).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while takeover.slot.received.load(Ordering::Relaxed) < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("takeover's eager replacement ACKs stopped repaint capture");
    assert!(takeover.slot.acknowledgements.load(Ordering::Relaxed) > 0);
    takeover.stop();
    browser.shutdown().await.unwrap();
}
