#!/bin/sh
set -eu
project='Orca Bridge'
tagline='Find the session. Keep the flow.'
color=false
if [ -z "${NO_COLOR+x}" ]; then
    case "${FORCE_COLOR:-}" in
        0) ;;
        ?*) color=true ;;
        *) if [ -t 1 ] && [ "${TERM:-dumb}" != dumb ]; then color=true; fi ;;
    esac
fi
mint= amber= ink= muted= bold= reset=
if [ "$color" = true ]; then
    esc=$(printf '\033')
    mint="${esc}[38;2;84;221;187m"
    amber="${esc}[38;2;255;179;77m"
    ink="${esc}[38;2;237;230;212m"
    muted="${esc}[38;2;118;138;144m"
    bold="${esc}[1m"
    reset="${esc}[0m"
fi
columns=${COLUMNS:-}
if [ -z "$columns" ] && [ -t 1 ] && command -v tput >/dev/null 2>&1; then
    columns=$(tput cols 2>/dev/null || true)
fi
case "$columns" in ''|*[!0-9]*) columns=80 ;; esac
if [ "$columns" -lt 32 ]; then columns=32; fi
printf '\n'
if [ "$columns" -lt 72 ]; then
    printf '  %s%s%s%s\n' "$bold" "$mint" "$project" "$reset"
else
    printf '  %s┌─ ◆ ─────────────────────────────────────────────────────────────┐%s\n' "$mint" "$reset"
    printf '    %s%s%s%s  %s//%s  %s%s%s\n' "$bold" "$ink" "$project" "$reset" "$muted" "$reset" "$amber" "$tagline" "$reset"
    printf '  %s└─────────────────────────────────────────────────────────────────┘%s\n' "$muted" "$reset"
fi
awk -v target_color="$mint" -v detail_color="$ink" -v heading="$amber" \
    -v bold="$bold" -v reset="$reset" -v columns="$columns" '
    /^##@ / { group=substr($0,5); next }
    /^[a-zA-Z0-9_-]+:.*## / {
        target=$0; sub(/:.*/, "", target)
        detail=$0; sub(/^[^:]+:.*##[[:space:]]*/, "", detail)
        if (seen[target]++) next
        count++; targets[count]=target; details[count]=detail; groups[count]=group
        if (length(target)>width) width=length(target)
    }
    END {
        if(width<16) width=16
        if(width>columns/2) width=int(columns/2)
        previous="__initial__"
        for(row=1;row<=count;row++) {
            if(groups[row]!=previous) {
                title=groups[row]; if(title=="") title="Commands"
                printf "\n  %s%s%s%s\n",bold,heading,title,reset
                previous=groups[row]
            }
            printf "  %s%-*s%s  ",target_color,width,targets[row],reset
            available=columns-width-4; if(available<10) available=10
            n=split(details[row],words,/ +/); line=""
            for(i=1;i<=n;i++) {
                if(length(line)+length(words[i])+1>available && line!="") {
                    printf "%s%s%s\n%*s",detail_color,line,reset,width+4,""
                    line=""
                }
                if(line!="") line=line " "
                line=line words[i]
            }
            printf "%s%s%s\n",detail_color,line,reset
        }
    }
' "$@"
printf '\n  %smake <target>%s  %sto run an action.%s\n\n' "$amber" "$reset" "$muted" "$reset"
