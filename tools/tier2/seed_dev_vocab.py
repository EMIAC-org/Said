#!/usr/bin/env python3
"""Seed the Said DB with developer vocabulary and realistic STT distortions.

Inserts terms into `vocabulary` and distortions into `stt_replacements`
so the ONNX trainer has a rich dataset to learn from.

Usage:
    python3 tools/tier2/seed_dev_vocab.py --db ~/Library/Application\ Support/VoicePolish/db.sqlite
"""

import argparse
import sqlite3
import time

DEV_TERMS = [
    # (term, term_type, meaning, distortions[])
    ("EMIAC", "acronym", "Company name", [
        "meac", "meic", "emiak", "emiag", "emiach", "amiac", "imiac",
    ]),
    ("Macobs", "proper_noun", "Company name", [
        "macops", "mecobs", "mcorbs", "makobs", "maccobs", "macabs", "makops",
    ]),
    ("n8n", "code_identifier", "Workflow automation tool", [
        "aneten", "naten", "nateen", "netn", "enten",
    ]),
    ("Kubernetes", "brand", "Container orchestration platform", [
        "cubinatis", "cubernetize", "cubernetis", "kubernetis", "cubernatis", "kubnatis", "kubernates",
    ]),
    ("Docker", "brand", "Container runtime", [
        "doker", "dockor", "dokar", "doccur", "dauker", "dokker",
    ]),
    ("PostgreSQL", "brand", "Relational database", [
        "postgress", "postgreskul", "postgrace", "postgresal", "posgressql", "postgrescue",
    ]),
    ("Redis", "brand", "In-memory data store", [
        "reedis", "redis", "rediss", "rehdis", "reddis", "raydis",
    ]),
    ("TypeScript", "brand", "JavaScript superset", [
        "typescript", "typscript", "taipscript", "typeskript", "typscript", "taypscript",
    ]),
    ("JavaScript", "brand", "Programming language", [
        "javascript", "javascrip", "javaskript", "jawascript", "javascripth",
    ]),
    ("GraphQL", "brand", "Query language for APIs", [
        "grafql", "graphkul", "graphquel", "graffql", "graphqul",
    ]),
    ("OAuth", "acronym", "Authentication protocol", [
        "oauth", "oaath", "oarth", "oaut", "oaff",
    ]),
    ("JWT", "acronym", "JSON Web Token", [
        "jwt", "jaywtee", "jot", "jdublyuti", "jewt",
    ]),
    ("Vercel", "brand", "Deployment platform", [
        "wersel", "vercell", "versel", "wercell", "vurcel",
    ]),
    ("Supabase", "brand", "Firebase alternative", [
        "superbase", "supabes", "supabas", "sooperbase", "supabaes",
    ]),
    ("Prisma", "brand", "ORM for Node.js", [
        "prisma", "prizma", "prism", "prissma", "prizma",
    ]),
    ("MongoDB", "brand", "NoSQL database", [
        "mongodb", "mongodebe", "mongodbi", "mongdb", "mangodb",
    ]),
    ("GitHub", "brand", "Code hosting", [
        "github", "githab", "gitub", "gethub", "githob",
    ]),
    ("GitLab", "brand", "DevOps platform", [
        "gitlab", "gitlub", "gitlaab", "getlab", "gitlob",
    ]),
    ("WebSocket", "brand", "Real-time protocol", [
        "websocket", "vebsocket", "websockt", "websokket", "websoket",
    ]),
    ("localhost", "code_identifier", "Local dev server", [
        "localhost", "localhoast", "localhosst", "lokalhost", "locahost",
    ]),
    ("kubectl", "code_identifier", "Kubernetes CLI", [
        "kubctl", "kubecontrol", "kubectel", "kubektl", "kubctl",
    ]),
    ("SQLite", "brand", "Embedded database", [
        "sqlite", "sqlight", "esqlite", "squlite", "sqllight",
    ]),
    ("Tauri", "brand", "Desktop app framework", [
        "tauri", "tawri", "touri", "taori", "towri",
    ]),
    ("Vite", "brand", "Frontend build tool", [
        "vite", "veet", "vait", "vaite", "vait",
    ]),
    ("React", "brand", "UI library", [
        "react", "reakt", "riact", "reeact", "reakth",
    ]),
    ("Next.js", "brand", "React framework", [
        "nextjs", "nekstjs", "nexjs", "nextjass", "nexjess",
    ]),
    ("Terraform", "brand", "Infrastructure as code", [
        "terraform", "teraform", "terrafoam", "terrafarm", "tereform",
    ]),
    ("Grafana", "brand", "Monitoring dashboard", [
        "grafana", "graffana", "grafna", "graphana", "grufana",
    ]),
    ("Prometheus", "brand", "Monitoring system", [
        "prometheus", "promethius", "promatheus", "prometheous", "promethus",
    ]),
    ("Ansible", "brand", "Configuration management", [
        "ansible", "ansibel", "ansibal", "ansble", "ansibul",
    ]),
    ("Jenkins", "brand", "CI/CD server", [
        "jenkins", "jenkinz", "jenkns", "jenks", "jinkins",
    ]),
    ("Nginx", "brand", "Web server", [
        "nginx", "enginex", "enginx", "nginks", "inginx",
    ]),
    ("Ubuntu", "brand", "Linux distribution", [
        "ubuntu", "ubunti", "ubanto", "oobuntu", "ubontu",
    ]),
    ("Firebase", "brand", "App platform by Google", [
        "firebase", "fayerbase", "firebas", "firbase", "fyrebase",
    ]),
    ("Figma", "brand", "Design tool", [
        "figma", "phigma", "fikma", "figmah", "feegma",
    ]),
    ("Stripe", "brand", "Payment processing", [
        "stripe", "straip", "strype", "stryp", "striep",
    ]),
    ("Twilio", "brand", "Communication APIs", [
        "twilio", "twilo", "twileo", "twillyo", "twillio",
    ]),
    ("Datadog", "brand", "Monitoring service", [
        "datadog", "datadogg", "datadoog", "datadag", "datdog",
    ]),
    ("Sentry", "brand", "Error tracking", [
        "sentry", "sentri", "sentre", "sentary", "sintry",
    ]),
    ("Cloudflare", "brand", "CDN and security", [
        "cloudflare", "cloudfliar", "cloudflair", "cloudfare", "cloudflayer",
    ]),
    ("Tailwind", "brand", "CSS framework", [
        "tailwind", "tailvind", "tailwnd", "taelwind", "tailwynd",
    ]),
    ("Svelte", "brand", "Frontend framework", [
        "svelte", "sfelt", "svelt", "swelte", "zvelte",
    ]),
    ("Angular", "brand", "Frontend framework", [
        "angular", "angulr", "anguler", "anglar", "angular",
    ]),
    ("Vue", "brand", "Frontend framework", [
        "vue", "voo", "vyoo", "vew", "vyu",
    ]),
    ("Django", "brand", "Python web framework", [
        "django", "jango", "dyango", "jeango", "djanggo",
    ]),
    ("FastAPI", "brand", "Python API framework", [
        "fastapi", "fastapie", "fastapy", "phastapi", "fastappi",
    ]),
    ("Celery", "brand", "Task queue", [
        "celery", "celry", "sellery", "celeri", "selery",
    ]),
    ("RabbitMQ", "brand", "Message broker", [
        "rabbitmq", "rabitmq", "rabbitmque", "rabitimq", "rabbitmk",
    ]),
    ("Elasticsearch", "brand", "Search engine", [
        "elasticsearch", "elasticsrch", "elastisearch", "elasticsearsh", "elastcsearch",
    ]),
    ("Kafka", "brand", "Event streaming", [
        "kafka", "kafkah", "kufka", "kafca", "kaffka",
    ]),
    ("RBAC", "acronym", "Role-based access control", [
        "rbac", "arbac", "rbak", "arback", "erbac",
    ]),
    ("API", "acronym", "Application Programming Interface", [
        "api", "aapi", "apie", "epi", "ape",
    ]),
    ("CI/CD", "acronym", "Continuous Integration/Delivery", [
        "cicd", "seeeyeceedee", "ciside", "sisid", "cised",
    ]),
]


def get_user_id(conn: sqlite3.Connection) -> str:
    row = conn.execute("SELECT id FROM local_user LIMIT 1").fetchone()
    if not row:
        raise SystemExit("no user found in DB")
    return row[0]


def seed(conn: sqlite3.Connection, user_id: str) -> None:
    now_ms = int(time.time() * 1000)
    vocab_inserted = 0
    alias_inserted = 0

    for term, term_type, meaning, distortions in DEV_TERMS:
        # Upsert vocabulary
        existing = conn.execute(
            "SELECT weight, use_count FROM vocabulary WHERE user_id = ? AND term = ?",
            (user_id, term),
        ).fetchone()

        if existing:
            # Bump weight if already exists
            conn.execute(
                "UPDATE vocabulary SET weight = MAX(weight, 3.0), use_count = MAX(use_count, 5), "
                "term_type = ?, meaning = ?, last_used = ? WHERE user_id = ? AND term = ?",
                (term_type, meaning, now_ms, user_id, term),
            )
        else:
            conn.execute(
                "INSERT INTO vocabulary (user_id, term, weight, use_count, last_used, source, "
                "term_type, meaning) VALUES (?, ?, 3.0, 5, ?, 'auto', ?, ?)",
                (user_id, term, now_ms, term_type, meaning),
            )
            vocab_inserted += 1

        # Insert STT replacement aliases for each distortion
        for distortion in distortions:
            norm_distortion = distortion.lower().strip()
            norm_term = term.lower().strip()
            if norm_distortion == norm_term:
                continue

            existing_alias = conn.execute(
                "SELECT 1 FROM stt_replacements WHERE user_id = ? AND transcript_form = ? AND correct_form = ?",
                (user_id, distortion, term),
            ).fetchone()

            if not existing_alias:
                # Simple phonetic key
                pkey = "".join(
                    ch for ch in norm_distortion if ch.isalpha()
                ).replace("c", "k").replace("q", "k").replace("v", "f")

                conn.execute(
                    "INSERT INTO stt_replacements (user_id, transcript_form, correct_form, "
                    "phonetic_key, weight, use_count, last_used, review_status) "
                    "VALUES (?, ?, ?, ?, 1.0, 1, ?, 'approved')",
                    (user_id, distortion, term, pkey, now_ms),
                )
                alias_inserted += 1

    conn.commit()
    print(f"Seeded {vocab_inserted} new vocab terms, {alias_inserted} new STT aliases")
    print(f"Total vocab: {conn.execute('SELECT COUNT(*) FROM vocabulary WHERE user_id = ?', (user_id,)).fetchone()[0]}")
    print(f"Total aliases: {conn.execute('SELECT COUNT(*) FROM stt_replacements WHERE user_id = ?', (user_id,)).fetchone()[0]}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", required=True, help="Path to Said's SQLite database")
    args = parser.parse_args()

    conn = sqlite3.connect(args.db)
    user_id = get_user_id(conn)
    print(f"Seeding for user: {user_id}")
    seed(conn, user_id)
    conn.close()


if __name__ == "__main__":
    main()
