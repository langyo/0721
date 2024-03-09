use gloo::storage::{LocalStorage, Storage};
use stylist::{css, yew::styled_component};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{app::Routes, functions::api::auth::refresh};
use _database::types::language_config::load_config;

#[styled_component]
pub fn Header() -> HtmlResult {
    let navigator = use_navigator().unwrap();
    let t = load_config().unwrap();

    let auth = use_state(|| None);

    use_effect_with((), {
        let navigator = navigator.clone();
        let auth = auth.clone();
        move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match refresh().await {
                    Ok(info) => {
                        auth.set(Some(info));
                    }
                    Err(_) => {
                        navigator.push(&Routes::Login);
                    }
                }
            });
            || {}
        }
    });

    Ok(html! {
        <>
            <div class={css!("
                position: fixed;
                width: 100vw;
                height: 100vh;
                left: 0;
                top: 0;

                background: url('/bg.webp') center / cover no-repeat;
                filter: blur(4px);
                z-index: -1;
                pointer-events: none;
            ")} />

            <header class={css!("
                position: fixed;
                width: 100%;
                height: 96px;
                left: 0;
                top: 0;

                display: flex;
                align-items: center;
                justify-content: flex-start;

                background: var(--color-background-header);
                z-index: 1000;
            ")}>
                <h1
                    class={css!("
                        width: max-content;
                        height: 96px;
                        margin-left: 24px;

                        color: var(--color-light);
                        font-size: 24px;
                        font-weight: bolder;
                        line-height: 96px;
                        text-decoration: none;
                        cursor: pointer;
                    ")}
                    onclick={move |_| {
                        gloo::utils::window().location().set_href("/").unwrap();
                    }}
                >
                    // TODO - Custom the text by global config
                    {"Ciallo～(∠·ω< )⌒★"}
                </h1>

                <aside class={css!("
                    position: absolute;
                    right: 0;
                    top: 0;
                    height: 100%;
                    padding: 24px;

                    display: flex;
                    align-items: center;
                    justify-content: center;
                ")}>
                    {
                        if let Some(info) = (*auth).clone() {
                            html! {
                                <>
                                    <span class={css!("
                                        color: var(--color-light);
                                        font-size: 16px;
                                        line-height: 32px;
                                        margin-right: 16px;

                                        user-select: none;

                                        @media (max-width: 991px) {
                                            display: none;
                                        }
                                    ")}>
                                        {t.header.welcome}
                                        <p class={css!("
                                            display: inline;
                                            font-weight: bolder;
                                        ")}>
                                            {info.name}
                                        </p>
                                    </span>
                                    <button
                                        class={css!("
                                            height: 32px;
                                            padding: 0 16px;
                                            margin: 8px;

                                            @media (max-width: 991px) {
                                                display: none;
                                            }
                                        ")}
                                        onclick={move |_| {
                                            LocalStorage::delete("token");
                                            gloo::utils::window().location().reload().unwrap();
                                        }}
                                    >
                                        {t.header.logout}
                                    </button>
                                </>
                            }
                        } else {
                            html! {
                                <button
                                    class={css!("
                                        height: 32px;
                                        padding: 0 16px;
                                        margin: 8px;

                                        @media (max-width: 991px) {
                                            display: none;
                                        }
                                    ")}
                                    onclick={move |_| {
                                        gloo::utils::window().location().set_href("/login").unwrap();
                                    }}
                                >
                                    {t.header.login}
                                </button>
                            }
                        }
                    }
                </aside>
            </header>
        </>
    })
}
