# Configuration

Source: environment variables; `.env` in working directory loaded if present (server, CLI, MCP).

## Environment {#environment}

| var | binary | default | effect |
|---|---|---|---|
| `DATABASE_URL` | all | required | Postgres URL |
| `HOST` | `solidate-server` | `127.0.0.1`; image `0.0.0.0` | bind address |
| `PORT` | `solidate-server` | `3000` | bind port |
| `SOLIDATE_PUBLIC_URL` | `solidate-server` | unset (relative links) | `llms.txt` base; MCP allowed host |
| `SOLIDATE_INSECURE_COOKIES` | `solidate-server` | unset | `1` -> non-`Secure` session cookie |
| `SOLIDATE_OIDC_ISSUER` | `solidate-server` | unset (SSO off) | OIDC issuer; discovery at startup, failure aborts start |
| `SOLIDATE_OIDC_CLIENT_ID` | `solidate-server` | required if issuer set | client ID |
| `SOLIDATE_OIDC_CLIENT_SECRET` | `solidate-server` | unset (public client) | `client_secret_basic` |
| `SOLIDATE_OIDC_REDIRECT_URL` | `solidate-server` | `SOLIDATE_PUBLIC_URL` + `/login/oidc/callback` | registered redirect URI; one of the two required |
| `SOLIDATE_OIDC_SCOPES` | `solidate-server` | `email profile` | space-separated; `openid` always added |
| `SOLIDATE_OIDC_LABEL` | `solidate-server` | `SSO` | sign-in button text |
| `SOLIDATE_RATE_LIMIT_PER_MIN` | server, `solidate-mcp` | `600` | per-token sustained rate; `0` disables |
| `SOLIDATE_RATE_LIMIT_BURST` | server, `solidate-mcp` | `60` | bucket size |
| `RUST_LOG` | all | server `info`; CLI, MCP `warn` | tracing filter |
| `SOLIDATE_LOG_FORMAT` | all | text | `json` -> JSON lines |
| `SOLIDATE_PASSWORD` | `solidate` | stdin | `user create`, `user passwd` |
| `SOLIDATE_TOKEN` | `solidate-mcp` | required | acting token |

## Built-in limits {#limits}

- `max_doc_bytes`: 1 MiB (1048576) per variant.
- `min_password_len`: 8.
- session lifetime: 14 days.
- include depth: 8.
- not runtime-configurable.

## Logging {#logging}

- all logs -> stderr (stdout = CLI output / MCP stdio).
- per request: method, path, status, duration.

## Running behind TLS {#tls}

- terminate TLS upstream; set `SOLIDATE_PUBLIC_URL` to public origin; unset `SOLIDATE_INSECURE_COOKIES`.
- rate limiter is in-process: effective limit = limit x replicas.
