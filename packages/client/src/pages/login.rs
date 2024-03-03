use log::{error, info};
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

use gloo::storage::{LocalStorage, Storage};
use stylist::css;
use yew::prelude::*;

use crate::functions::api::auth::{login, verify};
use _database::types::language_config::load_config;

#[function_component]
pub fn Login() -> HtmlResult {
    let t = load_config().unwrap();

    let is_verifying = use_state(|| true);

    let name_raw = use_state(|| "".to_string());
    let password_raw = use_state(|| "".to_string());

    #[rustfmt::skip]
    let input_style = css!("
        width: 80%;
        height: 48px;
        margin-top: 16px;
        padding: 0 16px;

        border: 1px solid #ccc;
        border-radius: 4px;
        outline: none;

        background: var(--color-light);
        box-shadow: var(--shadow-half);

        color: var(--color-dark-most);
        line-height: 48px;
        font-size: 16px;

        cursor: text;
    ");

    {
        let is_verifying = is_verifying.clone();

        use_effect_with((), {
            let is_verifying = is_verifying.clone();
            move |_| {
                wasm_bindgen_futures::spawn_local(async move {
                    match verify().await {
                        Ok(_) => {
                            gloo::utils::window().location().set_href("/").unwrap();
                        }
                        Err(err) => {
                            error!("Verify failed: {:?}", err);
                            LocalStorage::delete("token");
                        }
                    }

                    is_verifying.set(false);
                });
                || {}
            }
        });
    }

    Ok(html! {
        <div class={css!("
            padding: 64px 0;

            display: flex;
            flex-direction: column;
            align-items: center;
            justify-content: center;
        ")}>
            <div class={css!("
                position: relative;
                width: 400px;
                height: 100%;
                padding: 16px;
                padding-bottom: 64px;

                display: flex;
                flex-direction: column;
                align-items: center;
                justify-content: center;

                background: var(--color-light-half);
                border-radius: 8px;
                box-shadow: var(--shadow-half);
                backdrop-filter: blur(4px);
            ")}>
                <h1 class={css!("
                    height: 48px;
                    margin: 16px;

                    line-height: 48px;
                    text-align: center;

                    font-size: 24px;
                    font-weight: bolder;
                    color: var(--color-primary);
                    user-select: none;
                ")}>
                    { t.header.login.clone() }
                </h1>

                <input
                    class={input_style.clone()}
                    type="text"
                    placeholder={t.header.username}
                    value={(*name_raw).clone()}
                    oninput={{
                        let name_raw = name_raw.clone();
                        Callback::from(move |e: InputEvent| {
                            let target = e.target();
                            let input = target.and_then(|target| target.dyn_into::<HtmlInputElement>().ok());

                            if let Some(input) = input {
                                name_raw.set(input.value());
                            }
                        })
                    }}
                />
                <input
                    class={input_style.clone()}
                    type="password"
                    placeholder={t.header.password}
                    value={(*password_raw).clone()}
                    oninput={{
                        let password_raw = password_raw.clone();
                        Callback::from(move |e: InputEvent| {
                            let target = e.target();
                            let input = target.and_then(|target| target.dyn_into::<HtmlInputElement>().ok());

                            if let Some(input) = input {
                                password_raw.set(input.value());
                            }
                        })
                    }}
                />

                <button
                    class={classes!(css!("
                        margin-top: 16px;
                        width: 60%;
                        height: 48px;

                        background: var(--color-light);
                        border: none;
                        border-radius: 4px;
                        outline: none;
                        box-shadow: var(--shadow-half);

                        color: var(--color-dark-most);
                        font-size: 16px;
                        line-height: 48px;
                        text-align: center;

                        cursor: pointer;
                        transition: all 0.3s;

                        &:hover {
                            background: var(--color-primary);
                            color: var(--color-light);
                        }

                        &:active {
                            filter: brightness(0.8);
                        }
                    "), {
                        if *is_verifying {
                            css!("
                                background: var(--color-dark-half);
                                cursor: not-allowed;
                                pointer-events: none;
                            ")
                        } else {
                            css!("")
                        }
                    })}
                    onclick={{
                        let is_verifying = is_verifying.clone();
                        let name_raw = name_raw.clone();
                        let password_raw = password_raw.clone();

                        Callback::from(move |_| {
                            is_verifying.set(true);
                            let is_verifying = is_verifying.clone();

                            let name_raw = (*name_raw).clone();
                            let password_raw = (*password_raw).clone();

                            wasm_bindgen_futures::spawn_local(async move {
                                match login(name_raw, password_raw).await {
                                    Ok(info) => {
                                        info!("Login success: {:?}", info);

                                        gloo::utils::window().location().set_href("/").unwrap();
                                    }
                                    Err(err) => {
                                        error!("Login failed: {:?}", err);
                                        gloo::dialogs::alert(&format!("Login failed: {:?}", err));
                                    }
                                }

                                is_verifying.set(false);
                            });
                        })
                    }}
                >
                    {{
                        if *is_verifying {
                            t.header.loading
                        } else {
                            t.header.login.clone()
                        }
                    }}
                </button>
            </div>
        </div>
    })
}
