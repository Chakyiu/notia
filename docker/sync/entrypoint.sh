#!/bin/sh
# Docker creates a new volume as root. The server runs as the unprivileged
# `notia` user (uid 10001), so a root-owned /data accepts health checks and
# then rejects the first upload with "Permission denied (os error 13)".
# This process starts as root, gives the data directory to `notia`, and
# replaces itself with the server running as that user.
set -eu

data=/data
prev=
for arg in "$@"; do
  if [ "$prev" = "--data" ]; then
    data=$arg
  fi
  prev=$arg
done

drop_privs() {
  # setpriv execs the command, so the server is pid 1 and receives SIGTERM.
  exec setpriv --reuid=notia --regid=notia --init-groups --inh-caps=-all -- "$@"
}

if [ "$(id -u)" = 0 ]; then
  mkdir -p "$data"
  if ! chown -R notia:notia "$data"; then
    echo "notia-sync: could not give $data to uid 10001. On the host, run: chown -R 10001:10001 <the data directory>" >&2
  fi
  drop_privs /usr/local/bin/notia-sync-server "$@"
fi

exec /usr/local/bin/notia-sync-server "$@"
