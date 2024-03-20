use gloo::storage::{LocalStorage, Storage};
use stylist::{css, yew::styled_component};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    app::Routes,
    functions::api::auth::refresh,
    utils::global_state::{GlobalStateAction, GlobalStateContext},
};
use _database::types::{
    config::{load_config, Config},
    i18n::load_i18n,
};

#[styled_component]
pub fn Header() -> HtmlResult {
    let route = use_route::<Routes>().unwrap();
    let navigator = use_navigator().unwrap();
    let t = load_i18n().unwrap();
    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let global_config = use_prepared_state!((), async move |_| -> Option<Config> {
        if let Ok(ret) = load_config() {
            return Some(ret);
        }
        None
    })?
    .unwrap();
    let title_suffix = (*global_config)
        .clone()
        .map(|config| config.title_suffix)
        .unwrap_or("Ciallo～(∠·ω< )⌒★".to_string());

    use_effect_with((), {
        let route = route.clone();
        let navigator = navigator.clone();
        let global_state = global_state.clone();
        move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match refresh().await {
                    Ok(info) => {
                        global_state.dispatch(GlobalStateAction::SetToken(Some(info)));
                    }
                    Err(_) => {
                        if route != Routes::Login {
                            navigator.push(&Routes::Login);
                            gloo::utils::window().location().reload().unwrap();
                        }
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
                    onclick={
                        let navigator = navigator.clone();
                        move |_| {
                            navigator.push(&Routes::Portal);
                            gloo::utils::window().location().reload().unwrap();
                        }
                    }
                >
                    {title_suffix}
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
                        if let Some(info) = global_state.token.clone() {
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
                                        onclick={
                                            let navigator = navigator.clone();
                                            move |_| {
                                                LocalStorage::delete("auth");
                                                navigator.push(&Routes::Login);
                                                gloo::utils::window().location().reload().unwrap();
                                            }
                                        }
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
                                    onclick={
                                        let navigator = navigator.clone();
                                        move |_| {
                                            navigator.push(&Routes::Login);
                                            gloo::utils::window().location().reload().unwrap();
                                        }
                                    }
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
