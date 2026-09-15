# Anna's Kobo

Search Anna's Archive (and LibGen) from your Kobo, download for free, get a
KEPUB in your library a few seconds later. One tap in NickelMenu, everything
runs on the device, no computer involved.

Built and tested on a Kobo Clara BW, firmware 4.45. Rust, one static binary
of about 4 MB.

## Using it

1. NickelMenu → **Anna's Kobo**. A pop-up opens with a search box.
2. Type a title or author, pick a format (EPUB by default), tap **Search**.
   The page shows "Searching…" and refreshes itself; results take 5 to 10 s.
3. Tap **Download** on a result. The Downloads page shows progress:
   finding a link, downloading, converting to kepub.
4. When it says done, the Kobo browser shows "File Download … Continue".
   Tap **Continue**. Nickel adds the book to the library within 2 to 3 s.
5. Close the pop-up. The book is on Home and in My Books.

Failed downloads have a **Retry** button (mirrors return HTTP 500 now and
then; the app already walks five of them, three rounds, before giving up).
"Add to library again" makes a second copy, on purpose.

**Settings** (top bar): optional Anna's Archive membership key (enables
Anna's fast servers; without one, books come from LibGen's free servers),
fixed mirrors, search source, delivery mode, kepub on/off.

## Install

You need [NickelMenu](https://pgaskin.net/NickelMenu/) on the Kobo.

### Over USB (no ssh needed)

1. Build: `tools/build` (needs Rust, `zig`, `cargo-zigbuild`, and the
   `armv7-unknown-linux-musleabihf` target; see Development).
2. Plug the Kobo in. On the `KOBOeReader` drive create `.adds/annas-kobo/`
   and copy in `target/armv7-unknown-linux-musleabihf/release/annas-kobo`
   and `kobo/annas-kobo.sh`.
3. Append the contents of `kobo/nm-config` to `.adds/nm/config`.
4. Eject. NickelMenu picks up the new entry on its next open.

### Over Wi-Fi (developer route)

Enable ssh once: on the USB drive rename `.kobo/ssh-disabled` to
`.kobo/ssh-enabled`, eject, reboot. The first `ssh root@<kobo-ip>` asks you
to set a root password (scripted sessions hang on that prompt until it is
answered). Put your public key in `/.ssh/authorized_keys` on the device.

```sh
tools/build     # cross-compile
tools/deploy    # serve the binary over HTTP, Kobo pulls it with wget,
                # installs the launcher and the NickelMenu lines, restarts
```

`tools/deploy` reads `KOBO_HOST` (default 192.168.0.215), `KOBO_KEY`
(default `~/.ssh/id_ed25519_personal`) and `MAC_IP` from the environment.

Signing out of the Kobo account wipes `.kobo`, including the ssh marker and
the root password. Everything under `.adds` survives.

## How it works

- **Search.** Anna's Archive first. Its search pages currently sit behind a
  DDoS-Guard JavaScript check that a non-browser client cannot pass (a free
  account's login cookie does not skip it either, tested 2026-09-15). When
  the check appears the app remembers it for six hours and searches LibGen
  instead, which serves the same files under the same md5 identifiers, and
  says so on the results page. Searches run in the background; the page
  polls.
- **Download.** With a membership key: Anna's `dyn/api/fast_download.json`
  (the only Anna's endpoint not behind the check). Otherwise, or if that
  fails: LibGen's `ads.php?md5=` page → `get.php` link → CDN. Anna's own
  slow downloads need the browser check, a wait and a captcha, so they are
  not used.
- **Kepub.** EPUBs are converted on the device with
  [kepub-rs](https://github.com/tiago-cos/kepub-rs), a Rust port of
  [kepubify](https://github.com/pgaskin/kepubify): about 2 s per book on the
  Clara BW. Kobo's reader then gets fast page turns, stats and proper
  highlights.
- **Into the library.** Finished books wait in RAM (`/tmp/annas-kobo`) and
  the Downloads page hands them to the Kobo browser as an attachment
  (`/file/<id>`). Nickel's browser controller (`N3BrowserControllerBase::
  startDownload → parseDownloadedFiles → onDownloadedFileSynced`) saves the
  file to the storage root and runs `N3FSSyncManager::sync`, the same call
  its Google Drive integration uses. Measured: 2 to 3 s from hand-off to
  "Side-loading … as a book" in Nickel's log. The books must not wait under
  `/mnt/onboard`: Nickel scans every folder there, dot-folders included,
  and would import the waiting copy too (it also imports any `.txt`, which
  is why the log is `annas-kobo.log`).
- **Folder mode** (Settings) saves to `/mnt/onboard/Books` instead. Nickel
  then needs a rescan: a NickelMenu entry
  `menu_item:main:Import books:nickel_misc:rescan_books` (shows the
  blocking "Importing content" dialog) or NickelDBus. The old fake-USB-plug
  trick through `/tmp/nickel-hardware-status` only shows the connect dialog
  on firmware 4.45 and imports nothing.
- **Mirrors.** Picked from [open-slum.org](https://open-slum.org/) in order
  of reported health, probed, cached for 30 minutes, warmed at startup.
  Overrides in Settings.
- **Loopback.** Nickel never brings `lo` up, so the launcher runs
  `ifconfig lo up`; without it the pop-up sits on "Loading…" forever.

The UI is plain server-rendered HTML for the Kobo's old WebKit: big
buttons, black on white, meta-refresh for progress, no JavaScript.

## Files on the device

```
/mnt/onboard/.adds/annas-kobo/annas-kobo      the service (listens on 0.0.0.0:8484)
/mnt/onboard/.adds/annas-kobo/annas-kobo.sh   launcher: lo up, start service, wait for it
/mnt/onboard/.adds/annas-kobo/config.json     settings (also editable over USB)
/mnt/onboard/.adds/annas-kobo/annas-kobo.log  log
/tmp/annas-kobo/                              finished books waiting for the browser (RAM)
/mnt/onboard/Books/                           downloads in folder mode
```

Diagnostics from any machine on the LAN: `http://<kobo-ip>:8484/log`
(app log) and `http://<kobo-ip>:8484/syslog?n=500` (Nickel's syslog).

## Command line

```
annas-kobo serve [--data-dir DIR] [--listen HOST:PORT]
annas-kobo search [--source auto|annas|libgen] [--ext epub] QUERY
annas-kobo download MD5
annas-kobo kepubify FILE...
annas-kobo mirrors
```

All of it runs on a Mac too (`cargo run -- serve`, then open
http://127.0.0.1:8484/), which is how the UI is developed.

## Development

```sh
brew install rustup zig && rustup toolchain install stable
rustup target add armv7-unknown-linux-musleabihf
cargo install cargo-zigbuild

cargo test                                  # parser tests
cargo run -- search --ext epub dune herbert
cargo run -- kepubify book.epub
tools/build && tools/deploy
echo 'tail -30 /mnt/onboard/.adds/annas-kobo/annas-kobo.log' | tools/kobo-sh
tools/kobo-shot shot.png                    # screenshot (needs koboterm on the device)
```

`tools/kobo-sh` feeds a script to the Kobo through the stock sshd's
interactive shell (it serves no commands, scp or sftp). Cold-cache network
work on the device can take a minute; give it time.

## Layout

| file | role |
|---|---|
| `src/main.rs` | CLI, logger |
| `src/web.rs`, `src/pages.rs` | HTTP server, background searches, HTML pages |
| `src/annas.rs` | Anna's Archive search parsing, challenge detection, fast download API |
| `src/libgen.rs` | LibGen search parsing and free download links |
| `src/slum.rs` | open-slum.org parsing, mirror probing, cache, blocked-state memory |
| `src/jobs.rs` | download queue, mirror walking, file naming, conversion, hand-off |
| `src/kepub.rs` | EPUB → KEPUB via kepub-rs |
| `src/nickel.rs` | Nickel detection, NickelDBus rescan for folder mode |
| `src/config.rs` | `config.json` |
| `kobo/` | launcher script and NickelMenu lines |
| `tools/` | build, deploy, kobo-sh, kobo-shot |
