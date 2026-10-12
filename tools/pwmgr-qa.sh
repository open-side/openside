#!/usr/bin/env bash
# C-7 / PP-19: password-manager extension compatibility against a local Vaultwarden.
#
#   tools/pwmgr-qa.sh up        download extensions + desktop app, start Vaultwarden (127.0.0.1 only), mock site
#   tools/pwmgr-qa.sh run       run every stage for the stock and patched browser (all headless)
#   tools/pwmgr-qa.sh down      stop and remove the container, mock server, desktop app and temp dirs
#   tools/pwmgr-qa.sh all       up, run, down (down also runs on failure)
#
# Environment:
#   OPENSIDE_PATCHED_BROWSER  patched chrome (stages for it are skipped when unset)
#   STOCK_BROWSER             defaults to /usr/bin/chromium
#   PWMGR_WORK / PWMGR_OUT    scratch (removed by `down`) and evidence (kept) directories
#   STAGES                    space separated subset of: static headless native onepassword
#
# Needs docker, bun, python3 (+ cryptography), openssl and curl. Never uses a privileged container; the server is
# published on 127.0.0.1 only. Chromium profiles and HOME trees are throw-away; the user's own profiles, keyring and
# desktop are never touched. Every browser here is headless.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
Q="$ROOT/tools/pwmgr-qa"
export PWMGR_WORK="${PWMGR_WORK:-/tmp/openside-qa/pwmgr-work}"
export PWMGR_OUT="${PWMGR_OUT:-/tmp/openside-qa/pwmgr}"
W="$PWMGR_WORK"
STOCK_BROWSER="${STOCK_BROWSER:-/usr/bin/chromium}"
CONTAINER=openside-qa-pwmgr-vw
VAULT_PORT=18222
MOCK_PORT=18223
BW_EXT=nngceckbapebfimnlniiiahkandclblb
OP_EXT=aeblfdkhhhdcdjpifhhbdiojplfjncoa
BW_DESKTOP=https://github.com/bitwarden/clients/releases/download/desktop-v2026.9.1/bitwarden_2026.9.1_x64.tar.gz
STAGES="${STAGES:-static headless native onepassword}"

crx() { # name id
    [[ -f "$W/crx/$1.crx" ]] || curl -fsSL -m 180 -o "$W/crx/$1.crx" \
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=152.0&acceptformat=crx3&x=id%3D$2%26uc"
    python3 "$Q/unpack_crx.py" "$W/crx/$1.crx" "$W/ext/$1" "$2"
}

up() {
    mkdir -p "$W"/{crx,ext,dl,tls,vw-data} "$PWMGR_OUT"
    crx bitwarden "$BW_EXT"
    crx 1password "$OP_EXT"
    # nativeMessaging is an optional permission whose prompt cannot be answered headlessly; the copy asks for it up front.
    rm -rf "$W/ext/bitwarden-nm" && cp -r "$W/ext/bitwarden" "$W/ext/bitwarden-nm"
    python3 - "$W/ext/bitwarden-nm/manifest.json" <<'EOF'
import json, sys
path = sys.argv[1]
manifest = json.load(open(path))
manifest["permissions"].append("nativeMessaging")
json.dump(manifest, open(path, "w"))
EOF
    if [[ ! -x "$W/desktop/bitwarden-app" ]]; then
        [[ -f "$W/dl/bw-desktop.tar.gz" ]] || curl -fsSL -m 900 -o "$W/dl/bw-desktop.tar.gz" "$BW_DESKTOP"
        mkdir -p "$W/desktop" && tar -xzf "$W/dl/bw-desktop.tar.gz" -C "$W/desktop"
    fi
    # The Bitwarden extension refuses http:// server URLs, so Vaultwarden gets a throw-away self-signed cert
    # that the QA browsers trust by SPKI hash only (--ignore-certificate-errors-spki-list).
    if [[ ! -f "$W/tls/cert.pem" ]]; then
        openssl req -x509 -newkey rsa:2048 -nodes -keyout "$W/tls/key.pem" -out "$W/tls/cert.pem" -days 2 \
            -subj "/CN=127.0.0.1" -addext "subjectAltName=IP:127.0.0.1,DNS:localhost" 2>/dev/null
        chmod 644 "$W/tls/key.pem"
        openssl x509 -in "$W/tls/cert.pem" -pubkey -noout | openssl pkey -pubin -outform der \
            | openssl dgst -sha256 -binary | base64 >"$W/tls/spki.b64"
    fi
    cp "$W/tls/cert.pem" "$W/tls/key.pem" "$W/vw-data/"
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    docker run -d --name "$CONTAINER" -p "127.0.0.1:$VAULT_PORT:80" -v "$W/vw-data:/data" \
        -e SIGNUPS_ALLOWED=true -e "DOMAIN=https://127.0.0.1:$VAULT_PORT" -e LOG_LEVEL=warn \
        -e 'ROCKET_TLS={certs="/data/cert.pem",key="/data/key.pem"}' vaultwarden/server:latest >/dev/null
    for _ in $(seq 1 40); do curl -skf -m 2 "https://127.0.0.1:$VAULT_PORT/alive" >/dev/null && break; sleep 0.5; done
    curl -skf -m 2 "https://127.0.0.1:$VAULT_PORT/alive" >/dev/null
    if [[ ! -f "$W/creds.json" ]]; then
        python3 - "$W/creds.json" <<'EOF'
import json, os, secrets, sys
fd = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, json.dumps({"email": "qa-pwmgr@openside.invalid", "master": secrets.token_urlsafe(24),
                         "site_user": "qa-user@example.test", "site_pass": "S3cret-" + secrets.token_hex(8)}).encode())
EOF
    fi
    read -r EMAIL MASTER SUSER SPASS < <(python3 -c "import json;c=json.load(open('$W/creds.json'));print(c['email'],c['master'],c['site_user'],c['site_pass'])")
    python3 "$Q/register.py" "https://127.0.0.1:$VAULT_PORT" "$EMAIL" "$MASTER" 2>&1 | grep -v "^$" || true
    python3 "$Q/add_login.py" "https://127.0.0.1:$VAULT_PORT" "$EMAIL" "$MASTER" "openside QA mock login" \
        "http://127.0.0.1:$MOCK_PORT/login.html" "$SUSER" "$SPASS"
    if ! curl -sf -m 2 "http://127.0.0.1:$MOCK_PORT/login.html" >/dev/null; then
        setsid python3 -m http.server "$MOCK_PORT" --bind 127.0.0.1 --directory "$Q/mock" >/dev/null 2>&1 &
        echo $! >"$W/mock.pid"
        sleep 1
    fi
}

run() {
    mkdir -p "$PWMGR_OUT"
    : >"$PWMGR_OUT/results.jsonl"
    local browsers=("stock:$STOCK_BROWSER")
    [[ -n "${OPENSIDE_PATCHED_BROWSER:-}" ]] && browsers+=("patched:$OPENSIDE_PATCHED_BROWSER")
    for stage in $STAGES; do
        if [[ "$stage" == static ]]; then bun "$Q/run.mjs" static || true; continue; fi
        for entry in "${browsers[@]}"; do
            bun "$Q/run.mjs" "$stage" --label "${entry%%:*}" --browser "${entry#*:}" || true
        done
    done
    python3 - "$PWMGR_OUT/results.jsonl" <<'EOF'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1])]
for row in rows:
    print(f"{row['status']:5} {row['label']:8} {row['id']:6} {row['check']}")
counts = {}
for row in rows:
    counts[row['status']] = counts.get(row['status'], 0) + 1
print(counts)
sys.exit(1 if counts.get('fail') else 0)
EOF
}

down() {
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    [[ -f "$W/mock.pid" ]] && kill "$(cat "$W/mock.pid")" 2>/dev/null || true
    pkill -f "$W/" 2>/dev/null || true
    pkill -f "http.server $MOCK_PORT --bind 127.0.0.1 --directory $Q/mock" 2>/dev/null || true
    rm -rf "$W"
}

case "${1:-}" in
    up) up ;;
    run) run ;;
    down) down ;;
    all) trap down EXIT; up; run ;;
    *) sed -n '2,20p' "$0"; exit 2 ;;
esac
