#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::{login_fixture::LoginFixture, Harness};
use openside_proto::EventKind;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

pub const OWNER: &str = "web-visual-acceptance";
pub const LONG_TITLE: &str = "https://example.test/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const KOREAN_TITLE: &str = "다음 주 팀 워크숍 장소 예약과 참석자 일정 조율, 그리고 발표 자료 초안까지 한꺼번에 정리해 달라는 요청에 대한 긴 대화";

pub fn text(value: &str) -> Value {
    json!({"parts":[{"type":"text","text":value}]})
}
pub fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}

pub struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    pub session: String,
    next: u64,
}
impl Cdp {
    pub async fn attach(browser: &Browser, target: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(&browser.endpoint().ws_url)
            .await
            .unwrap();
        let mut cdp = Self {
            socket,
            session: String::new(),
            next: 0,
        };
        cdp.session = cdp
            .call(
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
            )
            .await["sessionId"]
            .as_str()
            .unwrap()
            .into();
        cdp
    }
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let mut request = json!({"id":id,"method":method,"params":params});
        if !self.session.is_empty() {
            request["sessionId"] = json!(self.session);
        }
        self.socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(30), self.socket.next())
                .await
                .expect("CDP reply")
                .unwrap()
                .unwrap();
            if let Message::Text(body) = frame {
                let reply: Value = serde_json::from_str(body.as_str()).unwrap();
                if reply["id"] == id {
                    assert!(reply["error"].is_null(), "{method}: {reply}");
                    return reply["result"].clone();
                }
            }
        }
    }
    pub async fn viewport(&mut self, width: u32, height: u32, theme: &str) {
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":width <= 390}),
        )
        .await;
        self.call("Emulation.setEmulatedMedia", json!({"features":[{"name":"prefers-color-scheme","value":theme},{"name":"prefers-reduced-motion","value":"reduce"}]})).await;
        self.call(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":0,"y":0}),
        )
        .await;
    }
    pub async fn eval(&mut self, expression: &str) -> Value {
        let reply = self.call("Runtime.evaluate", json!({"expression":expression,"awaitPromise":true,"returnByValue":true,"userGesture":true})).await;
        assert!(reply["exceptionDetails"].is_null(), "{expression}: {reply}");
        reply["result"]["value"].clone()
    }
    pub async fn wait(&mut self, expression: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if self.eval(expression).await == true {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "waiting for {expression}: {}",
                self.eval("document.body.innerText").await
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    pub async fn capture(&mut self, path: &Path) {
        use base64::Engine;
        self.eval("document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(true)))))").await;
        let shot = self
            .call(
                "Page.captureScreenshot",
                json!({"format":"png","captureBeyondViewport":false}),
            )
            .await;
        std::fs::write(
            path,
            base64::engine::general_purpose::STANDARD
                .decode(shot["data"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
    }
}

pub struct Ui {
    pub browser: Browser,
    pub tab: String,
    pub origin: String,
    pub cdp: Cdp,
}
impl Ui {
    pub async fn open(
        executable: PathBuf,
        harness: &Harness,
        token: &str,
        extensions: Vec<PathBuf>,
    ) -> Self {
        let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
        let origin = format!("http://127.0.0.1:{}", port.trim());
        let mut config = BrowserConfig::new(executable);
        config.extensions = extensions;
        let (browser, _) = Browser::launch(config).await.unwrap();
        let tab = browser
            .create_agent_tab(OWNER, &format!("{origin}/"))
            .await
            .unwrap()
            .id;
        let cdp = Cdp::attach(&browser, &tab).await;
        let mut ui = Self {
            browser,
            tab,
            origin,
            cdp,
        };
        ui.cdp
            .wait("!!document.querySelector('.token-entry')")
            .await;
        ui.cdp.eval(&format!("localStorage.setItem('openside.token', {}); localStorage.setItem('openside.theme', 'system'); true", json!(token))).await;
        ui
    }
    pub async fn goto(&mut self, path: &str, ready: &str) {
        self.browser
            .navigate(OWNER, &self.tab, &format!("{}{path}", self.origin))
            .await
            .unwrap();
        self.cdp.wait(ready).await;
        self.cdp.eval("document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(true)))))").await;
    }
}

pub struct Fixture {
    pub harness: Harness,
    pub site: LoginFixture,
    pub session: String,
    pub token: String,
    pub executable: PathBuf,
    pub tickets: Vec<Value>,
}
impl Fixture {
    pub async fn start() -> Self {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dist = repo.join("web/dist");
        assert!(
            dist.join("index.html").is_file(),
            "build web/dist before visual acceptance"
        );
        let executable = openside_config::discover_browser(&openside_config::Config::default())
            .expect("visual acceptance requires Chromium");
        let reply = format!("The page lives at {LONG_TITLE}\n\n`{}`\n\n```text\n{}\n```\n\n| URL | Result |\n| --- | --- |\n| {LONG_TITLE} | Ready |", "x".repeat(160), "y".repeat(400));
        let script = json!({"responses":[tool("task_start", json!({"goal":format!("Compare {LONG_TITLE}")}), "visual-task"), text(&reply), text("The comparison is ready.")],
            "subagents":{"visual-task":[text(&format!("Task report: {LONG_TITLE}"))]}});
        let mut harness = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script)
            .expect("uv sync --project agent is required");
        let config = harness.root().join("config/openside/config.toml");
        let original = std::fs::read_to_string(&config).unwrap();
        std::fs::write(
            config,
            original.replace("orchestration = \"single\"", "orchestration = \"front\""),
        )
        .unwrap();
        harness.append_config(&format!(
            "[ui]\ndir = {}\n[browser]\nexecutable = {}\n[skills]\ndir = {}\n",
            json!(dist.canonicalize().unwrap()),
            json!(executable),
            json!(harness.root().join("skill-library"))
        ));
        harness.launch().await;
        assert_eq!(
            harness
                .request("PUT", "/v1/onboarding", Some(json!({"state":"completed"})))
                .await
                .status,
            200
        );
        let site = LoginFixture::start().await;
        let session = create(
            &harness,
            &LONG_TITLE[..openside_proto::MAX_SESSION_TITLE_CHARS],
        )
        .await;
        create(&harness, KOREAN_TITLE).await;
        create(&harness, "Short title").await;
        submit(&harness, &session, "initial", "Summarize this page").await;
        tokio::time::timeout(
            Duration::from_secs(60),
            harness.events(&session, 0).await.settled(),
        )
        .await
        .expect("initial reply and Task settle");
        let paired = harness.request("POST", "/v1/devices/pair", Some(json!({"name":"visual-acceptance","scopes":["read","steer","approve","human_step","admin"]}))).await;
        assert_eq!(paired.status, 201, "{}", paired.body);
        Self {
            harness,
            site,
            session,
            token: paired.body["token"].as_str().unwrap().into(),
            executable,
            tickets: Vec::new(),
        }
    }
    pub async fn cards(&mut self) {
        for (path, name, kind) in [
            ("/login", "request_login", "login"),
            ("/profiles", "request_choice", "choice"),
            ("/verify/sms", "request_otp", "secret_input"),
            ("/push", "wait_for_approval", "device_approval"),
        ] {
            let opened = direct(
                &self.harness,
                &self.session,
                "browser_open_tab",
                json!({"url":format!("{}{path}", self.site.origin)}),
            )
            .await;
            assert_eq!(opened["status"], "done", "{opened}");
            let tab = &opened["result"]["tab_id"];
            let mut args = json!({"tab_id":tab});
            if name == "request_otp" {
                let snapshot = direct(
                    &self.harness,
                    &self.session,
                    "browser_snapshot",
                    json!({"tab_id":tab}),
                )
                .await;
                let snapshot = snapshot["result"].as_str().unwrap();
                let line = snapshot
                    .lines()
                    .find(|line| line.contains("textbox") && line.contains("code"))
                    .expect("code field");
                let start = line.find('[').unwrap() + 1;
                let end = line[start..].find(']').unwrap() + start;
                args["field_ref"] = json!(&line[start..end]);
                args["channel"] = json!("sms");
            }
            let card = direct(&self.harness, &self.session, name, args).await;
            assert_eq!(card["status"], "deferred", "{name}: {card}");
            let ticket = self
                .harness
                .request(
                    "GET",
                    &format!("/v1/tickets/{}", card["ticket_id"].as_str().unwrap()),
                    None,
                )
                .await;
            assert_eq!(ticket.status, 200);
            assert_eq!(ticket.body["kind"], kind, "{name}: {}", ticket.body);
            assert_eq!(ticket.body["status"], "pending");
            self.tickets.push(ticket.body);
        }
        let card = direct(
            &self.harness,
            &self.session,
            "request_human_step",
            json!({"kind":"confirm","summary":format!("Approve {LONG_TITLE}?")}),
        )
        .await;
        assert_eq!(card["status"], "deferred", "{card}");
        let ticket = self
            .harness
            .request(
                "GET",
                &format!("/v1/tickets/{}", card["ticket_id"].as_str().unwrap()),
                None,
            )
            .await;
        assert_eq!(ticket.status, 200);
        assert_eq!(ticket.body["kind"], "confirm");
        assert_eq!(ticket.body["status"], "pending");
        self.tickets.push(ticket.body);
    }
}

pub async fn create(harness: &Harness, title: &str) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":title,"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().into()
}
pub async fn submit(harness: &Harness, session: &str, id: &str, message: &str) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":id,"text":message,"mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}
pub async fn direct(harness: &Harness, session: &str, name: &str, args: Value) -> Value {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/tools/{name}"),
            Some(json!({"args":args})),
        )
        .await;
    assert_eq!(response.status, 200, "{name}: {}", response.body);
    response.body
}

pub async fn thinking_session(harness: &Harness) -> String {
    harness.replace_script(&json!({"responses":[{"parts":[{"type":"wait_steer","timeout_ms":1500},{"type":"text","text":"The answer is ready."}]}]}));
    create(harness, "Transient status acceptance").await
}

pub async fn assert_transient_status(ui: &mut Ui, harness: &Harness, session: &str) {
    ui.goto(
        &format!("/sessions/{session}"),
        "!!document.querySelector('.session')",
    )
    .await;
    submit(harness, session, "thinking", "Please answer").await;
    // Once the agent has started on the message, typing dots stand in for a text status.
    ui.cdp
        .wait("!!document.querySelector('.typing .typing-dot')")
        .await;
    let style = ui.cdp.eval("(() => { const el=document.querySelector('.typing'); const s=getComputedStyle(el); return {radius:parseFloat(s.borderRadius), height:el.getBoundingClientRect().height, background:s.backgroundColor}; })()").await;
    assert!(
        style["radius"].as_f64().unwrap() >= style["height"].as_f64().unwrap() / 2.0,
        "status is a pill: {style}"
    );
    ui.cdp.wait("document.body.innerText.includes('The answer is ready.') && !document.querySelector('.typing')").await;
    let mut stream = harness.events(session, 0).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while !matches!(stream.next().await.kind, EventKind::SessionSettled {}) {}
    })
    .await
    .unwrap();
}
