#!/usr/bin/env python3
"""Create a GitHub App through GitHub's manifest flow.

GitHub has no REST endpoint that creates an App. The official automated path is
the manifest flow:

  1. this script starts a localhost callback server and opens the manifest page;
  2. you click "Create GitHub App" in the browser (2FA stays with you);
  3. GitHub redirects back with a one-time code, which is exchanged for the
     App ID, private key and client secrets;
  4. the script writes them outside the repository and waits for the install
     redirect to record the installation ID.

Only the Python standard library is used. Nothing is sent anywhere except
api.github.com.

    python tools/github-app/create_app.py
    python tools/github-app/create_app.py --owner my-org --owner-type org
    python tools/github-app/create_app.py --dry-run
"""
from __future__ import annotations

import argparse
import html
import json
import os
import secrets
import sys
import threading
import time
import urllib.parse
import urllib.request
import webbrowser
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

GITHUB_API = "https://api.github.com"
DEFAULT_REPO = "Excurs1ons/PrismaRev"
DEFAULT_NAME = "prismrev-agent"


@dataclass
class SetupState:
    expected_state: str
    out_dir: Path
    manifest_url: str = ""
    manifest: dict = field(default_factory=dict)
    base_page: str = ""
    app: dict | None = None
    credentials: tuple[Path, Path] | None = None
    app_created: threading.Event = field(default_factory=threading.Event)
    installed: threading.Event = field(default_factory=threading.Event)


def exchange_code(code: str) -> dict:
    request = urllib.request.Request(
        f"{GITHUB_API}/app-manifests/{urllib.parse.quote(code)}/conversions",
        method="POST",
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "PrismaRev-app-setup",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def save_credentials(data: dict, out_dir: Path) -> tuple[Path, Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    pem_path = out_dir / "github-app.pem"
    pem_path.write_text(data["pem"], encoding="utf-8")
    pem_path.chmod(0o600)

    record = {
        "app_id": data["id"],
        "slug": data["slug"],
        "name": data.get("name", data["slug"]),
        "html_url": data.get("html_url"),
        "install_url": f"https://github.com/apps/{data['slug']}/installations/new",
        "private_key_path": str(pem_path),
        # Sensitive, kept only in this 0600 file. Installation tokens do not
        # need these, but they are useful if the App later uses OAuth/webhooks.
        "client_id": data.get("client_id"),
        "client_secret": data.get("client_secret"),
        "webhook_secret": data.get("webhook_secret"),
    }
    json_path = out_dir / "github-app.json"
    json_path.write_text(json.dumps(record, indent=2), encoding="utf-8")
    json_path.chmod(0o600)
    return pem_path, json_path


def record_installation(out_dir: Path, installation_id: int) -> None:
    path = out_dir / "github-app.json"
    record = json.loads(path.read_text(encoding="utf-8"))
    record["installation_id"] = installation_id
    path.write_text(json.dumps(record, indent=2), encoding="utf-8")
    path.chmod(0o600)


class CallbackHandler(BaseHTTPRequestHandler):
    server_version = "PrismaRevAppSetup/1.0"

    def log_message(self, fmt: str, *args) -> None:
        # Do not log callback codes or query strings.
        return

    @property
    def state(self) -> SetupState:
        return self.server.setup_state  # type: ignore[attr-defined]

    def page(self, status: int, title: str, body: str) -> None:
        document = (
            "<!doctype html><meta charset='utf-8'>"
            f"<title>{html.escape(title)}</title>"
            "<style>body{font:16px system-ui;max-width:42rem;margin:4rem auto;padding:0 1rem}"
            "code{background:#eee;padding:.15rem .3rem;border-radius:.2rem}</style>"
            f"<h1>{html.escape(title)}</h1>{body}"
        ).encode()
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(document)))
        self.end_headers()
        self.wfile.write(document)

    def do_GET(self) -> None:
        parsed = urllib.parse.urlparse(self.path)
        query = urllib.parse.parse_qs(parsed.query)

        if parsed.path == "/start":
            # GitHub's manifest flow expects a POST form, not a GET query
            # string. Serve a local page that submits the manifest for the user.
            manifest = json.dumps(self.state.manifest, separators=(",", ":"))
            action = f"{self.state.base_page}?state={urllib.parse.quote(self.state.expected_state)}"
            form = (
                f"<form action='{html.escape(action, quote=True)}' method='post'>"
                f"<input type='hidden' name='manifest' value='{html.escape(manifest, quote=True)}'>"
                "<button type='submit'>Continue to GitHub</button>"
                "</form>"
                "<script>document.forms[0].submit()</script>"
            )
            self.page(200, "Register GitHub App", "<p>Redirecting to GitHub...</p>" + form)
            return

        if parsed.path == "/callback":
            if self.state.app is not None:
                self.page(200, "Already created", "<p>This App was already created. You can close this tab.</p>")
                return
            if query.get("state", [None])[0] != self.state.expected_state:
                self.page(400, "State mismatch", "<p>The callback state did not match. Close this tab and rerun the script.</p>")
                return
            code = query.get("code", [None])[0]
            if not code:
                self.page(400, "Missing code", "<p>GitHub did not return a manifest code. Close this tab and rerun the script.</p>")
                return
            try:
                data = exchange_code(code)
                self.state.credentials = save_credentials(data, self.state.out_dir)
            except Exception as error:  # noqa: BLE001 - show the user, never the code
                print(f"ERROR: could not exchange the manifest code: {error}", file=sys.stderr)
                self.page(500, "Exchange failed", "<p>Could not exchange the code. See the terminal for details.</p>")
                return
            self.state.app = data
            self.state.app_created.set()
            install_url = f"https://github.com/apps/{data['slug']}/installations/new"
            self.page(
                200,
                "App created",
                f"<p><strong>{html.escape(data['slug'])}</strong> was created.</p>"
                f"<p><a href='{html.escape(install_url)}'>Install it on {html.escape(DEFAULT_REPO)}</a>, "
                "then come back to the terminal.</p>",
            )
            return

        if parsed.path == "/installed":
            installation_id = query.get("installation_id", [None])[0]
            if installation_id and installation_id.isdigit():
                record_installation(self.state.out_dir, int(installation_id))
                self.state.installed.set()
                self.page(
                    200,
                    "Installed",
                    f"<p>Installation <code>{installation_id}</code> recorded. You can close this tab.</p>",
                )
            else:
                self.page(400, "Missing installation", "<p>GitHub did not return an installation ID.</p>")
            return

        self.page(404, "Not found", "<p>Unknown callback path.</p>")


def build_manifest(args: argparse.Namespace, port: int) -> dict:
    base = f"http://localhost:{port}"
    return {
        "name": args.name,
        "url": f"https://github.com/{args.repo}",
        "redirect_url": f"{base}/callback",
        "callback_urls": [f"{base}/callback"],
        "setup_url": f"{base}/installed",
        "public": False,
        "default_permissions": {
            "issues": "write",
            "metadata": "read",
        },
        "default_events": [],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--owner", default="Excurs1ons", help="GitHub user or organization that will own the App")
    parser.add_argument("--owner-type", choices=("personal", "org"), default="personal")
    parser.add_argument("--name", default=DEFAULT_NAME, help="App name (must be unique on GitHub)")
    parser.add_argument("--repo", default=DEFAULT_REPO, help="Repository shown in the App URL")
    parser.add_argument(
        "--out",
        default=str(Path.home() / ".prismrev" / "github-app"),
        help="Directory for the private key and metadata (outside the repository)",
    )
    parser.add_argument("--port", type=int, default=0, help="Callback port (0 picks a free port)")
    parser.add_argument("--timeout", type=int, default=900, help="Seconds to wait for create/install")
    parser.add_argument("--no-browser", action="store_true", help="Only print the URL")
    parser.add_argument("--dry-run", action="store_true", help="Print the manifest URL and exit")
    args = parser.parse_args()

    out_dir = Path(args.out).expanduser()
    state = secrets.token_urlsafe(32)

    base_page = (
        f"https://github.com/organizations/{args.owner}/settings/apps/new"
        if args.owner_type == "org"
        else "https://github.com/settings/apps/new"
    )

    if args.dry_run:
        manifest = build_manifest(args, args.port or 8765)
        print(f"POST {base_page}?state={state}")
        print(json.dumps(manifest, indent=2))
        print()
        print("Manifest flow uses POST. Run without --dry-run to serve the local form.")
        return 0

    server = HTTPServer(("127.0.0.1", args.port), CallbackHandler)
    setup = SetupState(expected_state=state, out_dir=out_dir)
    server.setup_state = setup  # type: ignore[attr-defined]
    port = server.server_port
    setup.manifest = build_manifest(args, port)
    setup.base_page = base_page
    setup.manifest_url = f"http://localhost:{port}/start"

    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "create-url.txt").write_text(setup.manifest_url + "\n", encoding="utf-8")

    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    print("Local start page (form posts the manifest to GitHub):")
    print(setup.manifest_url)
    print()
    print(f"Callback server: http://localhost:{port}/callback")
    print(f"Credentials will be written to: {out_dir}")
    print("Waiting for you to click 'Create GitHub App' in the browser...\n")

    if not args.no_browser:
        webbrowser.open(setup.manifest_url)

    deadline = time.time() + args.timeout
    while time.time() < deadline and not setup.app_created.is_set():
        setup.app_created.wait(timeout=1)
    if setup.app is None:
        print("Timed out waiting for the App to be created. Rerun the script when ready.")
        server.shutdown()
        return 1

    assert setup.credentials is not None
    app = setup.app
    pem_path, json_path = setup.credentials
    install_url = f"https://github.com/apps/{app['slug']}/installations/new"
    print(f"App created: {app['slug']} (id {app['id']})")
    print(f"Private key:  {pem_path}")
    print(f"Metadata:     {json_path}")
    print(f"Install URL:  {install_url}")
    print("\nInstall it on the repository, then return here (waiting for the install redirect)...")

    remaining = max(1, int(deadline - time.time()))
    if setup.installed.wait(timeout=remaining):
        record = json.loads(json_path.read_text(encoding="utf-8"))
        print(f"Installation ID recorded: {record['installation_id']}")
    else:
        print("Install was not detected in time. You can install later; the bot can also discover it.")

    server.shutdown()
    print("\nNext: put these in the bot's environment (never in git):")
    print(f"  GITHUB_APP_ID={app['id']}")
    print(f"  GITHUB_APP_PRIVATE_KEY_PATH={pem_path}")
    print("  GITHUB_INSTALLATION_ID=<from github-app.json, or let the bot discover it>")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("\nCancelled.", file=sys.stderr)
        raise SystemExit(130)
