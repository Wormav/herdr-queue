#!/bin/sh
[ "$#" = 3 ] && [ "$1" = session ] && [ "$2" = list ] && [ "$3" = --json ] || exit 1
printf '%s\n' '{"sessions":[{"name":"work","socket_path":"/tmp/work.sock","running":true}]}'
