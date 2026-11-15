# Krypa web fixtures

Controlled test pages for the Krypa WebKit (WKWebView) mobile browser. Plain HTML, CSS and JavaScript. No frameworks, no build step, no external requests (except the `external-links` fixture, which links out on purpose).

Every page states on screen what it tests and what a correct result looks like. Open `index.html` for the list.

## Serving

Run these from this folder (`web-fixtures`). Use the Python already installed on the machine.

```
python -m http.server 8765
```

Open `http://localhost:8765/` on the device or simulator under test.

For the `cross-origin` fixture, start a second server on another port from the same folder. Two ports on the same host are two different origins:

```
python -m http.server 8766
```

Both servers must run at the same time. Stop them with Ctrl+C.

Notes:

- Serve over `http://localhost` (or `https`). Do not open the files with `file://`: cookies, service workers and most permission APIs do not work there.
- The device or simulator must reach the host. `localhost` only works for a browser on the same machine. For a phone, use the computer's LAN IP instead, for example `http://192.168.x.x:8765/`. A LAN IP is not a secure context, so `service-worker` and `permissions` will not work over plain http on a phone. Use `localhost` on a simulator or an HTTPS tunnel for those two.
- `python -m http.server` has no single-page-app fallback. `spa` therefore has one real `index.html` per deep-linked route (`/spa/`, `/spa/about/`, `/spa/items/42/`). Any other route returns 404 on reload, which is expected.
- Port 8799 must stay closed for the `errors` refused-connection link. Do not start anything on it.

## Which fixtures need localhost or HTTPS

| Fixture | Needs | Why |
| --- | --- | --- |
| `service-worker` | localhost or https (secure context) | `navigator.serviceWorker` is only available on secure origins. The page reports "unavailable" when it is missing. |
| `permissions` | localhost or https (secure context) | Geolocation, camera, microphone and notifications require a secure context. The page shows a warning when it is not one. |
| `cross-origin` | both port 8765 and port 8766 running | Origin B is `http://127.0.0.1:8766`. |
| `manifest-app` | any origin | Install behaviour is only meaningful on a secure origin, so use localhost or https for the real check. |
| `auth` | any origin, cookies enabled | Cookies are set for `/auth/`. |
| All others | any http origin | |

## Fixtures

| Fixture (folder) | What it tests | Expected correct result |
| --- | --- | --- |
| `basic/` (`index.html`, `second.html`, `third.html`, `long.html`) | Page navigation, same-page anchors, fragment links into a long page, scrolling | Links open the named pages. `#section-b` scrolls without reloading. `long.html#section-60` shows Section 60 at the top. |
| `multi-page/` (`index.html`, `a.html`, `b.html`, `c.html`) | Back/forward history, per-page visit counter in sessionStorage, back/forward cache | Back and forward show the right path. Counter goes up on a fresh load. A cache restore says so and does not change the counter. |
| `spa/` (`index.html`, `app.js`, `about/`, `items/42/`) | `history.pushState`, `popstate`, deep links on reload | Route links change the URL and view without a page load. Back and forward update the view. `/spa/about/` and `/spa/items/42/` work on reload. `/spa/no-such-route/` shows "not-found". |
| `forms/` (`index.html`) | Input types (text, email, password, number, date, textarea, select, checkbox, radio), autofocus, autocomplete, submit | Full name has focus on load. Required empty field is blocked. Submitted values appear on the page. Password is shown as a length only. Nothing is sent. |
| `auth/` (`index.html`) | Fake login, cookie and localStorage persistence, sign-out | Sign in with `tester` / `krypa-test-pass`: state "signed in", cookie and localStorage present. Still signed in after reload and a later visit. Sign out clears both. |
| `upload/` (`index.html`) | Single, multiple and image-capture file inputs | Choosing files lists name, size in bytes and type for each. Nothing is uploaded. |
| `media/` (`index.html`) | Audio and video with native controls, generated in the page | Audio: 2 s 440 Hz tone (WAV built from an OfflineAudioContext render) plays with controls. Video: about 2 s of canvas animation recorded with MediaRecorder plays. If either cannot be made, the status line says so on the page. No binary media files are shipped. |
| `manifest-app/` (`index.html`, `manifest.webmanifest`, `icon-192.svg`, `icon-512.svg`) | Valid web app manifest: name, short_name, start_url, scope, display standalone, theme and background colours, icons | The manifest status reads "parsed" and shows the values. The app installs as "Krypa Fixture App" and opens standalone at `index.html` in scope `/manifest-app/`. |
| `manifest-app/manifest-cross-origin.html` | Negative: start_url on another origin (`http://127.0.0.1:8766`) | The manifest is rejected or the start_url is ignored. The app never opens on the other origin. |
| `manifest-app/manifest-invalid.html` | Negative: manifest is not valid JSON (trailing comma, missing brace) | The manifest is ignored. The page still renders. No install metadata is used. |
| `manifest-app/manifest-rtl-name.html` | Negative: name contains U+202E right-to-left override (written as `‮` in the JSON) | The name is stored intact. The override does not reorder surrounding UI text, and the name is shown escaped or neutralised. |
| `service-worker/` (`index.html`, `sw.js`, `offline.html`, `probe.txt`) | Service worker registration, control, Cache Storage, offline navigation fallback | Registration becomes "active". After one reload the page is "controlled". The probe reports `cache` once cached. With the network off, `offline-check.html` shows the cached offline page. With `navigator.serviceWorker` missing, the page says "unavailable". |
| `external-links/` (`index.html`) | target=_blank, window.open, mailto:, tel:, custom app scheme, javascript: link, data: link | Each item's label states the safe behaviour. The new-window link opens elsewhere and leaves this page open. mailto and tel open the system app and can be cancelled. The custom scheme does nothing harmful. javascript: runs only inside this page. data: is blocked or rendered inert. **This fixture links out on purpose.** |
| `cross-origin/` (`index.html`, `redirect-a.html`, `redirect-b.html`, `frame-b.html`) | Navigation and redirect between origins (localhost:8765 and 127.0.0.1:8766), iframe from the other origin with postMessage | Origin B reachable reads "yes". The redirect lands on origin B after about 1.5 s. The iframe is ready. Ping gets a pong with the matching nonce. Iframe DOM access is blocked (null). Messages from other origins are ignored. |
| `errors/` (`index.html`, `memory.html`) | 404 page, refused connection, invalid hostname, manual memory use | The 404 shows the server's not-found result. The refused port shows a connection error. The `.invalid` host shows a DNS or not-found error. None of these shows a blank page. `memory.html` allocates 64 MB only on button press and never automatically. |
| `permissions/` (`index.html`) | Geolocation, camera, microphone and notification prompts, one at a time | Each button shows the browser prompt, then "granted" or "denied" (or the error name). Buttons are disabled while a request is pending. Missing APIs show "unavailable". |

## Automated tests

Use the `data-testid` attributes (or the `id` attributes) on each element. Common ones: `link-*`, `current-route`, `current-view`, `result-*`, `auth-state`, `single-output`, `audio-status`, `video-status`, `manifest-status`, `sw-reg-state`, `probe-source`, `origin-b-status`, `res-geo`.
