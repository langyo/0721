use log::{error, info};
use wasm_bindgen::JsCast as _;
use web_sys::HtmlInputElement;

use stylist::{css, yew::styled_component};
use yew::prelude::*;

use crate::{functions::models::user::register, utils::global_state::GlobalStateContext};
use _database::types::request::RegisterParams;

#[styled_component]
pub fn Register() -> HtmlResult {
    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let t = global_state.language.to_config().unwrap();

    let name_raw = use_state(|| "".to_string());
    let password_raw = use_state(|| "".to_string());
    let email_raw = use_state(|| "".to_string());

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

    Ok(html! {
        <div class={css!("
            position: relative;
            width: 100%;
            margin-top: 96px;
            margin-bottom: 64px;
            padding: 64px;

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
                    { t.header.register.clone() }
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
                    type="text"
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
                <input
                    class={input_style.clone()}
                    type="email"
                    placeholder={t.header.email}
                    value={(*email_raw).clone()}
                    oninput={{
                        let email_raw = email_raw.clone();
                        Callback::from(move |e: InputEvent| {
                            let target = e.target();
                            let input = target.and_then(|target| target.dyn_into::<HtmlInputElement>().ok());

                            if let Some(input) = input {
                                email_raw.set(input.value());
                            }
                        })
                    }}
                />

                <button
                    class={css!("
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
                    ")}
                    onclick={{
                        let name_raw = name_raw.clone();
                        let password_raw = password_raw.clone();
                        let email_raw = email_raw.clone();

                        Callback::from(move |_| {
                            let name = (*name_raw).clone();
                            let password_raw = (*password_raw).clone();
                            let email = (*email_raw).clone();

                            wasm_bindgen_futures::spawn_local(async move {
                                match register(&RegisterParams {
                                    name,
                                    password_raw,
                                    email,
                                    permission: "user".to_string(),
                                }).await {
                                    Ok(_) => {
                                        info!("Register success");
                                        gloo::dialogs::alert("Register success");
                                    }
                                    Err(err) => {
                                        error!("Register failed: {:?}", err);
                                        gloo::dialogs::alert(&format!("Register failed: {:?}", err));
                                    }
                                }
                            });
                        })
                    }}
                >
                    {t.header.register.clone()}
                </button>
            </div>
        </div>
    })
}
