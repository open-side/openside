use super::*;
use axum::{body::to_bytes, body::Body, extract::State, http::Request, Router};
use openside_skills::{Source, Store};
use std::path::Path;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

fn detail(enabled: bool) -> Value {
    json!({"name":"pdf-tools","description":"Extract text","source":"user","enabled":enabled,
        "license":null,"compatibility":null,"allowed_tools":[],"body":"Run pdftotext.\n",
        "resources":[{"path":"references/usage.md","size":11}]})
}

/// Fake daemon on a Unix socket: records requests and answers the skills endpoints.
async fn daemon(environment: &Environment) -> (PathBuf, Seen, JoinHandle<()>) {
    let socket = environment.directory.path().join("skills.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let router = Router::new()
        .fallback(
            |State(seen): State<Seen>, request: Request<Body>| async move {
                let method = request.method().to_string();
                let uri = request.uri().to_string();
                let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                seen.lock().push((method.clone(), uri.clone(), body.clone()));
                let answer = match (method.as_str(), uri.split('?').next().unwrap()) {
                    ("GET", "/v1/skills") => json!({
                        "skills":[{"name":"pdf-tools","description":"Extract text","source":"user","enabled":true}],
                        "problems":[{"name":"broken","source":"project","error":"missing frontmatter"}]}),
                    ("DELETE", _) => json!({"name":"pdf-tools"}),
                    ("PATCH", _) => detail(body["enabled"] == json!(true)),
                    _ => detail(true),
                };
                let status = if method == "POST" {
                    axum::http::StatusCode::CREATED
                } else {
                    axum::http::StatusCode::OK
                };
                (
                    status,
                    [("content-type", "application/json")],
                    answer.to_string(),
                )
            },
        )
        .with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (socket, seen, server)
}

async fn cli(socket: &Path, args: &[&str]) -> (u8, String, String) {
    let mut argv = vec![
        OsString::from("openside"),
        OsString::from("--socket"),
        socket.as_os_str().to_owned(),
    ];
    argv.extend(args.iter().map(OsString::from));
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run_with_input(argv, &mut std::io::empty(), &mut stdout, &mut stderr).await;
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

fn skill_dir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("references")).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Imported {name}\n---\nSteps\n"),
    )
    .unwrap();
    std::fs::write(dir.join("references/guide.md"), "guide").unwrap();
    dir
}

#[tokio::test]
async fn skills_commands_use_the_skills_api() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;

    let (code, out, err) = cli(&socket, &["skills", "list", "pdf", "--session", "s 1"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        "pdf-tools\tuser\tenabled\tExtract text\ninvalid\tbroken\tproject\tmissing frontmatter\n"
    );
    assert_eq!(seen.lock()[0].1, "/v1/skills?q=pdf&session=s+1");

    let (code, out, _) = cli(&socket, &["skills", "show", "pdf-tools"]).await;
    assert_eq!(code, 0);
    assert!(
        out.starts_with("pdf-tools (user, enabled)\nExtract text\n"),
        "{out}"
    );
    assert!(out.contains("Run pdftotext.") && out.contains("file\treferences/usage.md\t11"));
    assert_eq!(seen.lock()[1].1, "/v1/skills/pdf-tools");

    let (code, out, _) = cli(&socket, &["--json", "skills", "show", "pdf-tools"]).await;
    assert_eq!(code, 0);
    let first: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(first["body"], "Run pdftotext.\n");

    let (code, _, err) = cli(
        &socket,
        &[
            "skills",
            "new",
            "pdf-tools",
            "--description",
            "Extract text",
            "--body",
            "x",
        ],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        seen.lock().last().unwrap().2,
        json!({"name":"pdf-tools","description":"Extract text","body":"x","license":null,"resources":[]})
    );

    let (code, out, _) = cli(&socket, &["skills", "disable", "pdf-tools"]).await;
    assert_eq!(code, 0);
    assert!(out.starts_with("pdf-tools (user, disabled)"), "{out}");
    let (method, path, body) = seen.lock().last().unwrap().clone();
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("PATCH", "/v1/skills/pdf-tools")
    );
    assert_eq!(
        body,
        json!({"description":null,"body":null,"enabled":false})
    );
    let (_, out, _) = cli(&socket, &["skills", "enable", "pdf-tools"]).await;
    assert!(out.contains("enabled"), "{out}");

    let (code, out, _) = cli(&socket, &["skills", "rm", "pdf-tools"]).await;
    assert_eq!((code, out.as_str()), (0, "Deleted skill pdf-tools\n"));
    assert_eq!(seen.lock().last().unwrap().0, "DELETE");
    server.abort();
}

#[tokio::test]
async fn install_puts_the_browser_skill_into_an_agents_skills_directory() {
    let environment = Environment::new().await;
    let root = environment.directory.path().to_owned();
    let nowhere = root.join("no.sock");

    // Explicit directory.
    let target = root.join("custom-skills");
    let (code, out, err) = cli(
        &nowhere,
        &[
            "skills",
            "install",
            "claude",
            "--to-dir",
            target.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with("Installed skill openside-browser for claude at "),
        "{out}"
    );
    let installed = std::fs::read_to_string(target.join("openside-browser/SKILL.md")).unwrap();
    assert!(installed.starts_with("---\nname: openside-browser\n"));
    // What was installed is a valid skill that a library scan accepts.
    let scan = Store::new(&target).scan(None);
    assert!(scan.problems.is_empty());
    assert_eq!(scan.entries[0].skill.frontmatter.name, "openside-browser");
    assert_eq!(scan.entries[0].source, Source::User);

    // Reinstalling needs --force, and failure leaves the first copy alone.
    let (code, _, err) = cli(
        &nowhere,
        &[
            "skills",
            "install",
            "claude",
            "--to-dir",
            target.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 1);
    assert!(
        err.contains("already installed") && err.contains("--force"),
        "{err}"
    );
    let (code, ..) = cli(
        &nowhere,
        &[
            "skills",
            "install",
            "claude",
            "--to-dir",
            target.to_str().unwrap(),
            "--force",
        ],
    )
    .await;
    assert_eq!(code, 0);

    // Built-in conventions. The test environment points every agent variable at one
    // temporary directory, so give each agent its own place first.
    let codex_home = root.join("codex-home");
    let claude_home = root.join("claude-home");
    std::env::set_var("CODEX_HOME", &codex_home);
    std::env::set_var("CLAUDE_CONFIG_DIR", &claude_home);
    let (code, out, err) = cli(&nowhere, &["--json", "skills", "install", "codex"]).await;
    assert_eq!(code, 0, "{err}");
    let report: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(report["skill"], "openside-browser");
    assert!(codex_home
        .join("skills/openside-browser/SKILL.md")
        .is_file());
    let (code, _, err) = cli(&nowhere, &["skills", "install", "claude"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(claude_home
        .join("skills/openside-browser/SKILL.md")
        .is_file());

    // Without CLAUDE_CONFIG_DIR, Claude Code falls back to $HOME.
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    let home = root.join("fake-home");
    std::env::set_var("HOME", &home);
    let (code, _, err) = cli(&nowhere, &["skills", "install", "claude"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(home
        .join(".claude")
        .join("skills/openside-browser/SKILL.md")
        .is_file());

    // omp: its own environment variables first, then `$HOME/<its config dir>/skills`.
    std::env::remove_var("OMP_CODING_AGENT_DIR");
    std::env::remove_var("PI_CODING_AGENT_DIR");
    let (code, _, err) = cli(&nowhere, &["skills", "install", "omp"]).await;
    assert_eq!(code, 0, "{err}");
    let omp_default = home.join(".omp").join("agent").join("skills");
    assert!(omp_default.join("openside-browser/SKILL.md").is_file());
    let pi_dir = root.join("pi-agent");
    std::env::set_var("PI_CODING_AGENT_DIR", &pi_dir);
    let (code, _, err) = cli(&nowhere, &["skills", "install", "omp"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(pi_dir.join("skills/openside-browser/SKILL.md").is_file());
    let omp_dir = root.join("omp-agent");
    std::env::set_var("OMP_CODING_AGENT_DIR", &omp_dir);
    let (code, ..) = cli(&nowhere, &["skills", "install", "omp"]).await;
    assert_eq!(code, 0);
    assert!(omp_dir.join("skills/openside-browser/SKILL.md").is_file());

    // Agents without a built-in convention come from the environment ...
    let (code, _, err) = cli(&nowhere, &["skills", "install", "cursor"]).await;
    assert_eq!(code, 1);
    assert!(
        err.contains("OPENSIDE_SKILLS_CURSOR_DIR") && err.contains("[skills.agents]"),
        "{err}"
    );
    let via_env = root.join("cursor-skills");
    std::env::set_var("OPENSIDE_SKILLS_CURSOR_DIR", &via_env);
    let (code, ..) = cli(&nowhere, &["skills", "install", "cursor"]).await;
    std::env::remove_var("OPENSIDE_SKILLS_CURSOR_DIR");
    assert_eq!(code, 0);
    assert!(via_env.join("openside-browser/SKILL.md").is_file());

    // ... or from the configuration file.
    let config_dir = root.join("openside");
    std::fs::create_dir_all(&config_dir).unwrap();
    let configured = root.join("configured-skills");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "[skills.agents]\ngemini = {:?}\n",
            configured.to_str().unwrap()
        ),
    )
    .unwrap();
    let (code, _, err) = cli(&nowhere, &["skills", "install", "gemini"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(configured.join("openside-browser/SKILL.md").is_file());
}

#[tokio::test]
async fn import_copies_a_skill_into_the_library_and_refuses_scripts() {
    let environment = Environment::new().await;
    let root = environment.directory.path().to_owned();
    let nowhere = root.join("no.sock");
    let library = root.join("lib");
    std::fs::create_dir_all(root.join("openside")).unwrap();
    std::fs::write(
        root.join("openside/config.toml"),
        format!("[skills]\ndir = {:?}\n", library.to_str().unwrap()),
    )
    .unwrap();
    let source = root.join("other-agent");
    let dir = skill_dir(&source, "summarize");

    let (code, out, err) = cli(&nowhere, &["skills", "import", dir.to_str().unwrap()]).await;
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with("Imported skill summarize (2 files)"),
        "{out}"
    );
    assert_eq!(
        std::fs::read_to_string(library.join("summarize/references/guide.md")).unwrap(),
        "guide"
    );
    let (code, _, err) = cli(&nowhere, &["skills", "import", dir.to_str().unwrap()]).await;
    assert_eq!(code, 1);
    assert!(
        err.contains("already exists") && err.contains("--force"),
        "{err}"
    );
    let (code, ..) = cli(
        &nowhere,
        &["skills", "import", dir.to_str().unwrap(), "--force"],
    )
    .await;
    assert_eq!(code, 0);

    // By name from a skills directory.
    skill_dir(&source, "translate");
    let (code, _, err) = cli(
        &nowhere,
        &[
            "skills",
            "import",
            "translate",
            "--from-dir",
            source.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert!(library.join("translate/SKILL.md").is_file());

    // Executables are refused unless asked for, and nothing is left behind.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let scripted = skill_dir(&source, "scripted");
        let script = scripted.join("run.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (code, _, err) = cli(&nowhere, &["skills", "import", scripted.to_str().unwrap()]).await;
        assert_eq!(code, 1);
        assert!(err.contains("run.sh"), "{err}");
        assert!(!library.join("scripted").exists());
        let (code, ..) = cli(
            &nowhere,
            &[
                "skills",
                "import",
                scripted.to_str().unwrap(),
                "--allow-scripts",
            ],
        )
        .await;
        assert_eq!(code, 0);
        assert!(library.join("scripted/run.sh").is_file());
    }

    let (code, _, err) = cli(
        &nowhere,
        &["skills", "import", "missing", "--from", "cursor"],
    )
    .await;
    assert_eq!(code, 1);
    assert!(err.contains("OPENSIDE_SKILLS_CURSOR_DIR"), "{err}");

    // `source` is a skill name: it cannot climb out of the directory it is looked up in.
    let secret = skill_dir(&root.join("outside"), "stolen");
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    for hostile in ["../outside/stolen", secret.to_str().unwrap(), "a/b", ".."] {
        let (code, _, err) = cli(
            &nowhere,
            &[
                "skills",
                "import",
                hostile,
                "--from-dir",
                nested.to_str().unwrap(),
            ],
        )
        .await;
        assert_eq!(code, 1, "{hostile}");
        assert!(
            err.contains("name") || err.contains("invalid"),
            "{hostile}: {err}"
        );
        assert!(!library.join("stolen").exists(), "{hostile}");
        let (code, ..) = cli(&nowhere, &["skills", "import", hostile, "--from", "codex"]).await;
        assert_eq!(code, 1, "{hostile} via --from");
        assert!(!library.join("stolen").exists(), "{hostile} via --from");
    }
}
