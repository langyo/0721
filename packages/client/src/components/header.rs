use gloo::storage::{LocalStorage, Storage};
use stylist::{css, yew::styled_component};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    app::Routes,
    functions::api::auth::refresh,
    utils::global_state::{GlobalStateAction, GlobalStateContext},
};
use _database::types::config::{load_config, Config};

#[styled_component]
pub fn Header() -> HtmlResult {
    let route = use_route::<Routes>().unwrap();
    let navigator = use_navigator().unwrap();
    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let t = global_state.language.to_config().unwrap();
    let global_config = use_prepared_state!((), async move |_| -> Option<Config> {
        if let Ok(ret) = load_config() {
            return Some(ret);
        }
        None
    })?
    .unwrap();
    let title_suffix = (*global_config)
        .clone()
        .map(|config| config.portal.title_suffix)
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
                <a
                    class={css!("
                        width: max-content;
                        height: 96px;
                        margin-left: 24px;

                        color: var(--color-light-most);
                        font-size: 24px;
                        font-weight: bolder;
                        line-height: 96px;
                        text-decoration: none;

                        transition: all 0.3s;
                        cursor: pointer;

                        &:hover {
                            color: var(--color-light);
                        }
                    ")}
                    href={"/"}
                >
                    {title_suffix}
                </a>

                <nav class={css!("
                    margin-left: 24px;
                    margin-right: auto;

                    display: flex;
                    align-items: center;
                    justify-content: center;

                    & > a {
                        height: 32px;
                        padding: 0 16px;
                        margin: 12px;

                        color: var(--color-light-most);
                        font-size: 20px;
                        line-height: 32px;
                        text-decoration: none;

                        transition: all 0.3s;
                        cursor: pointer;
                    }

                    & > a:hover {
                        color: var(--color-light);
                    }
                ")}>
                    <a href={"/images"}>
                        {t.header.images}
                    </a>
                    <a href={"/users"}>
                        {t.header.users}
                    </a>
                    <a href={"/config"}>
                        {t.header.config}
                    </a>
                </nav>

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
