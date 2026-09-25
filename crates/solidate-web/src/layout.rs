use topcoat::Result;
use topcoat::context::Cx;
use topcoat::router::error::{BadRequestError, ForbiddenError, NotFoundError};
use topcoat::router::{Slot, StatusCode, layout};
use topcoat::view::{View, error_boundary, view};

use crate::assets::{app_js_url, css_url, htmx_url, mermaid_url};
use crate::auth::current_user;

#[layout("/")]
async fn root(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    let user = current_user(cx).await.map(|u| u.name.clone());
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Solidate"</title>
                <link rel="stylesheet" href=(css_url())>
                <script src=(htmx_url()) defer=""></script>
                <script src=(app_js_url()) data-mermaid=(mermaid_url()) defer=""></script>
            </head>
            <body>
                <header class="top">
                    <a class="brand" href="/">"Solidate"</a>
                    match user {
                        Some(name) => {
                            <span class="who">(name)</span>
                            <form method="post" action="/logout" class="inline">
                                <button type="submit" class="link">"Sign out"</button>
                            </form>
                        },
                        None => "",
                    }
                </header>
                <main>
                    error_boundary(
                        fallback: |error| {
                            let (status, title, detail) = if error.downcast_ref::<NotFoundError>().is_some() {
                                (StatusCode::NOT_FOUND, "Not found", String::new())
                            } else if error.downcast_ref::<ForbiddenError>().is_some() {
                                (StatusCode::FORBIDDEN, "Access denied", String::new())
                            } else if let Some(e) = error.downcast_ref::<BadRequestError>() {
                                (StatusCode::BAD_REQUEST, "Request rejected", e.to_string())
                            } else {
                                return Err(error);
                            };
                            Ok(view! {
                                (status)
                                <section class="error">
                                    <h1>(title)</h1>
                                    <p>(detail)</p>
                                    <p><a href="/">"Home"</a></p>
                                </section>
                            })
                        },
                        (slot)
                    )
                </main>
            </body>
        </html>
    })
}
