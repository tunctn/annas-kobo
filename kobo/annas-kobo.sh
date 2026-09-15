#!/bin/sh
# Anna's Kobo launcher. Installed at /mnt/onboard/.adds/annas-kobo/annas-kobo.sh
# and run by NickelMenu (cmd_output) before it opens the browser: starts the
# background service if it is not already answering, waits until it does.
DIR=/mnt/onboard/.adds/annas-kobo
PORT=8484
# Not .txt: Nickel scans all of /mnt/onboard and imports .txt files as books.
LOG=$DIR/annas-kobo.log
cd "$DIR" || exit 1
# Nickel never brings the loopback interface up; without it neither the
# browser nor this script can reach 127.0.0.1.
/sbin/ifconfig lo up 2>/dev/null || ifconfig lo up 2>/dev/null

alive() { wget -q -O /dev/null "http://127.0.0.1:$PORT/health" 2>/dev/null; }

if ! alive; then
  # Keep the log small.
  [ -f "$LOG" ] && [ "$(wc -c < "$LOG")" -gt 524288 ] && tail -c 65536 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"
  echo "=== start $(date)" >> "$LOG"
  setsid nohup "$DIR/annas-kobo" serve --data-dir "$DIR" >> "$LOG" 2>&1 < /dev/null &
  i=0
  while [ $i -lt 40 ]; do
    alive && break
    usleep 250000
    i=$((i + 1))
  done
fi
alive
