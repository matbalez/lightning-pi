#!/bin/sh
set -eu
mkdir -p /data/state
chown app:app /data/state
chmod 700 /data/state
exec gosu app /usr/local/bin/lightning-pi
