#!/bin/sh
# Cardea's link opener, installed as xdg-open (and BROWSER) in a sandbox whose links may leave: it hands an http or
# https link to the host through the sandbox's link channel, where it is checked again before any browser sees it.
fifo=/run/cardea/links
nl='
'
case $1 in
    *"$nl"*) echo "xdg-open: refused: the link has a line break" >&2; exit 1 ;;
    http://* | https://*) ;;
    *) echo "xdg-open: refused: only http and https links leave this sandbox" >&2; exit 1 ;;
esac
if [ "${#1}" -gt 4095 ]; then
    echo "xdg-open: refused: the link is too long" >&2
    exit 1
fi
[ -p "$fifo" ] || { echo "xdg-open: links cannot leave this sandbox" >&2; exit 1; }
# One write below PIPE_BUF is atomic: two apps opening links at once never mix their lines.
printf '%s\n' "$1" > "$fifo"
exit 0
