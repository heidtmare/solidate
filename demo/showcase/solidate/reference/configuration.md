# Configuration

Solidate is configured through environment variables. The server, the CLI and
the stdio MCP binary also read a `.env` file from the working directory when one
is present.

## Environment {#environment}

| Variable | Used by | Default | Purpose |
|---|---|---|---|
| `DATABASE_URL` | all | required | PostgreSQL connection string |
| `HOST` | server | `127.0.0.1` (`0.0.0.0` in the image) | Listen address |
| `PORT` | server | `3000` | Listen port |
| `SOLIDATE_PUBLIC_URL` | server | none | Absolute base URL for `llms.txt` links and the MCP host allow-list |
| `SOLIDATE_INSECURE_COOKIES` | server | off | Set to `1` for plain-HTTP development |
| `SOLIDATE_OIDC_ISSUER` | server | none | OpenID Connect issuer URL; enables single sign-on |
| `SOLIDATE_OIDC_CLIENT_ID` | server | required with issuer | Client ID registered with the provider |
| `SOLIDATE_OIDC_CLIENT_SECRET` | server | none | Client secret; omit for a public client |
| `SOLIDATE_OIDC_REDIRECT_URL` | server | `SOLIDATE_PUBLIC_URL` + `/login/oidc/callback` | Callback URL registered with the provider |
| `SOLIDATE_OIDC_SCOPES` | server | `email profile` | Scopes requested in addition to `openid` |
| `SOLIDATE_OIDC_LABEL` | server | `SSO` | Provider name on the sign-in button |
| `SOLIDATE_RATE_LIMIT_PER_MIN` | server, MCP | `600` | Sustained requests per token; `0` disables |
| `SOLIDATE_RATE_LIMIT_BURST` | server, MCP | `60` | Requests allowed at once |
| `RUST_LOG` | all | `info` (CLI, MCP: `warn`) | Log filter |
| `SOLIDATE_LOG_FORMAT` | all | text | `json` for one JSON object per line |
| `SOLIDATE_PASSWORD` | CLI | stdin | Password for `user create` and `user passwd` |
| `SOLIDATE_TOKEN` | MCP (stdio) | required | API token the process acts as |

## Built-in limits {#limits}

A single document variant may be at most 1 MiB. Passwords must be at least eight
characters. Browser sessions last fourteen days. Includes nest at most eight
levels deep. These are not configurable at runtime.

## Logging {#logging}

All binaries log to standard error, because standard output carries CLI results
and the stdio MCP protocol. Each HTTP request is logged with its method, path,
status and duration.

## Running behind TLS {#tls}

Single sign-on also needs `SOLIDATE_PUBLIC_URL` (or `SOLIDATE_OIDC_REDIRECT_URL`),
because the provider redirects the browser back to an absolute URL. The server
contacts the provider at startup and does not start if it is unreachable.

In production, terminate TLS in front of the server, set `SOLIDATE_PUBLIC_URL`
to the public origin, and leave `SOLIDATE_INSECURE_COOKIES` unset so session
cookies are marked `Secure`. Rate limits are enforced per process, so with
several replicas the effective limit is multiplied by the replica count.
