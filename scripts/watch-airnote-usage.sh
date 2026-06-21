#!/usr/bin/env bash
set -euo pipefail

interval="1"
duration=""
csv_path=""
extra_regex=""
once="0"
summary_path="$(mktemp -t airnote-usage-summary.XXXXXX)"
trap 'rm -f "$summary_path"' EXIT

usage() {
  cat <<'EOF'
Usage:
  scripts/watch-airnote-usage.sh [options]

Options:
  --interval SECONDS   Sample interval. Default: 1
  --duration SECONDS   Stop after this many seconds. Default: run until Ctrl+C
  --csv PATH           Append samples to a CSV file
  --include REGEX      Also include processes whose command matches REGEX
  --once               Print one sample and exit
  -h, --help           Show this help

Examples:
  scripts/watch-airnote-usage.sh
  scripts/watch-airnote-usage.sh --interval 0.5 --csv .context/airnote-usage.csv
  scripts/watch-airnote-usage.sh --duration 120 --csv .context/airnote-usage.csv
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --interval)
      interval="${2:?missing value for --interval}"
      shift 2
      ;;
    --duration)
      duration="${2:?missing value for --duration}"
      shift 2
      ;;
    --csv)
      csv_path="${2:?missing value for --csv}"
      shift 2
      ;;
    --include)
      extra_regex="${2:?missing value for --include}"
      shift 2
      ;;
    --once)
      once="1"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if ! command -v ps >/dev/null 2>&1; then
  echo "ps is required but was not found" >&2
  exit 1
fi

if [[ -n "$csv_path" ]]; then
  mkdir -p "$(dirname "$csv_path")"
  if [[ ! -f "$csv_path" ]]; then
    printf 'timestamp,pid,ppid,role,cpu_percent,mem_percent,rss_mb,command\n' > "$csv_path"
  fi
fi

csv_escape() {
  local value="${1//\"/\"\"}"
  printf '"%s"' "$value"
}

collect_rows() {
  ps axww -o pid= -o ppid= -o pcpu= -o pmem= -o rss= -o command= | \
    awk -v self="$$" -v extra="$extra_regex" '
      function classify(cmd) {
        if (cmd ~ /AirNote\.app\/Contents\/MacOS\/AirNote/ || cmd ~ /(^|[[:space:]])AirNote($|[[:space:]])/) return "airnote-app"
        if (cmd ~ /airnote-backend/ || cmd ~ /said-backend/) return "backend"
        if (cmd ~ /swift-stt-sidecar/ || cmd ~ /oriserve-swift/ || (cmd ~ /python/ && cmd ~ /server\.py/)) return "swift-local-stt"
        if (cmd ~ /whisper-cli/ || cmd ~ /whisper\.cpp/) return "whisper-local-stt"
        if (cmd ~ /VoicePolish\/models/ || cmd ~ /AirNote\/models/) return "model-worker"
        return "other"
      }

      {
        n += 1
        pid[n] = $1
        ppid[n] = $2
        cpu[n] = $3 + 0
        mem[n] = $4 + 0
        rss[n] = $5 + 0
        cmd = $0
        sub(/^[[:space:]]*[^[:space:]]+[[:space:]]+[^[:space:]]+[[:space:]]+[^[:space:]]+[[:space:]]+[^[:space:]]+[[:space:]]+[^[:space:]]+[[:space:]]+/, "", cmd)
        command[n] = cmd
        by_pid[pid[n]] = n

        role[n] = classify(cmd)
        if (pid[n] == self || cmd ~ /watch-airnote-usage\.sh/ || cmd ~ /awk -v self=/ || cmd ~ /^ps axww /) {
          role[n] = "ignored"
        }
        if (role[n] != "other" && role[n] != "ignored") {
          marked[n] = 1
        } else if (extra != "" && cmd ~ extra) {
          role[n] = "extra"
          marked[n] = 1
        }
      }

      END {
        changed = 1
        while (changed) {
          changed = 0
          for (i = 1; i <= n; i++) {
            parent = by_pid[ppid[i]]
            if (!marked[i] && parent && marked[parent] && role[i] != "ignored") {
              role[i] = "child-of-" role[parent]
              marked[i] = 1
              changed = 1
            }
          }
        }

        for (i = 1; i <= n; i++) {
          if (!marked[i]) continue
          printf "%s\t%s\t%s\t%.1f\t%.2f\t%.1f\t%s\n", pid[i], ppid[i], role[i], cpu[i], mem[i], rss[i] / 1024, command[i]
        }
      }
    '
}

print_sample() {
  local timestamp rows total_cpu total_rss
  timestamp="$(date '+%Y-%m-%dT%H:%M:%S%z')"
  rows="$(collect_rows)"

  printf '\n%s\n' "$timestamp"
  if [[ -z "$rows" ]]; then
    echo "No AirNote/local-model processes found. Open AirNote, select the local model, then start speaking."
    return
  fi

  printf '%-7s %-7s %-24s %8s %8s %9s %s\n' "PID" "PPID" "ROLE" "CPU%" "MEM%" "RSS_MB" "COMMAND"
  printf '%s\n' "$rows" | while IFS=$'\t' read -r pid ppid role cpu mem rss command; do
    printf '%-7s %-7s %-24s %8s %8s %9s %s\n' "$pid" "$ppid" "$role" "$cpu" "$mem" "$rss" "$command"
    printf '%s\t%s\t%s\t%s\t%s\n' "$pid" "$role" "$cpu" "$rss" "$command" >> "$summary_path"
    if [[ -n "$csv_path" ]]; then
      {
        csv_escape "$timestamp"; printf ','
        csv_escape "$pid"; printf ','
        csv_escape "$ppid"; printf ','
        csv_escape "$role"; printf ','
        csv_escape "$cpu"; printf ','
        csv_escape "$mem"; printf ','
        csv_escape "$rss"; printf ','
        csv_escape "$command"; printf '\n'
      } >> "$csv_path"
    fi
  done

  total_cpu="$(printf '%s\n' "$rows" | awk -F '\t' '{ total += $4 } END { printf "%.1f", total }')"
  total_rss="$(printf '%s\n' "$rows" | awk -F '\t' '{ total += $6 } END { printf "%.1f", total }')"
  printf '%-40s %8s %18s\n' "TOTAL" "${total_cpu}%" "${total_rss} MB RSS"
}

print_summary() {
  if [[ ! -s "$summary_path" ]]; then
    return
  fi

  printf '\nPeak summary\n'
  printf '%-7s %-24s %10s %11s %s\n' "PID" "ROLE" "MAX_CPU%" "MAX_RSS_MB" "COMMAND"
  awk -F '\t' '
    {
      key = $1 "\t" $2 "\t" $5
      if (($3 + 0) > max_cpu[key]) max_cpu[key] = $3 + 0
      if (($4 + 0) > max_rss[key]) max_rss[key] = $4 + 0
    }
    END {
      for (key in max_cpu) {
        split(key, parts, "\t")
        printf "%-7s %-24s %10.1f %11.1f %s\n", parts[1], parts[2], max_cpu[key], max_rss[key], parts[3]
      }
    }
  ' "$summary_path" | sort -k3,3nr
}

handle_interrupt() {
  print_summary
  exit 130
}

trap handle_interrupt INT TERM

start_epoch="$(date '+%s')"
echo "Watching AirNote app/backend/local-model processes. Press Ctrl+C to stop."
if [[ -n "$csv_path" ]]; then
  echo "CSV: $csv_path"
fi

while true; do
  print_sample

  if [[ "$once" == "1" ]]; then
    break
  fi

  if [[ -n "$duration" ]]; then
    now_epoch="$(date '+%s')"
    if (( now_epoch - start_epoch >= duration )); then
      break
    fi
  fi

  sleep "$interval"
done

print_summary
