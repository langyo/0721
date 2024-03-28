use std::collections::HashMap;

use stylist::{css, yew::styled_component};
use yew::prelude::*;

use crate::{
    components::icons,
    functions::models::user::{delete, list},
};

#[styled_component]
pub fn Users() -> HtmlResult {
    let is_downloading = use_state(|| true);
    let user_list = use_state(HashMap::new);
    use_effect_with((), {
        let is_downloading = is_downloading.clone();
        let user_list = user_list.clone();
        |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let user_list = user_list.clone();
                let ret = list().await.unwrap();
                user_list.set(ret);

                is_downloading.set(false);
            });
        }
    });

    Ok(html! {
        <>
            {
                if *is_downloading {
                    html! {
                        <div class={css!("
                            position: fixed;
                            width: 100vw;
                            height: 100vh;
                            left: 0;
                            top: 0;

                            display: flex;
                            flex-direction: column;
                            align-items: center;
                            justify-content: center;
                        ")}>
                            <icons::CircularProgress />
                        </div>
                    }
                } else {
                    html! {
                        <div class={css!("
                            position: relative;
                            width: 100%;
                            margin-top: 96px;
                            margin-bottom: 64px;
                            padding: 16px;

                            display: flex;
                            flex-direction: column;
                            align-items: center;
                            justify-content: center;
                        ")}>
                            {
                                user_list.iter().map(|(name, item)| html! {
                                    <div class={css!("
                                        width: 80%;
                                        height: 64px;
                                        margin: 4px 16px;
                                        padding: 16px;

                                        display: flex;
                                        align-items: center;
                                        justify-content: space-between;

                                        background: var(--color-light-less);
                                        border-radius: 4px;
                                        box-shadow: var(--shadow-half);
                                    ")}>
                                        <span class={css!("
                                            display: flex;
                                            flex-direction: column;
                                            align-items: flex-start;
                                            justify-content: center;

                                            font-size: 16px;
                                            user-select: none;
                                        ")}>
                                            <p class={css!("
                                                font-weight: bolder;
                                            ")}>
                                                {name.clone()}
                                            </p>
                                            <p>
                                                {item.email.clone()}
                                            </p>
                                        </span>
                                        <span class={css!("
                                            display: flex;
                                            align-items: center;
                                            justify-content: center;
                                        ")}>
                                            <button
                                                class={css!("
                                                    width: 32px;
                                                    height: 32px;
                                                    margin: 4px;

                                                    display: flex;
                                                    align-items: center;
                                                    justify-content: center;

                                                    background: var(--color-light-less);
                                                    border-radius: 4px;
                                                    box-shadow: var(--shadow-half);
                                                ")}
                                                onclick={
                                                    let name = name.clone();
                                                    Callback::from(move |_| {
                                                        gloo::utils::window().location().set_href(&format!("/register?name={}", name)).unwrap();
                                                    })
                                                }
                                            >
                                                <icons::AccountEdit size={16} />
                                            </button>
                                            <button
                                                class={css!("
                                                    width: 32px;
                                                    height: 32px;
                                                    margin: 4px;

                                                    display: flex;
                                                    align-items: center;
                                                    justify-content: center;

                                                    background: var(--color-light-less);
                                                    border-radius: 4px;
                                                    box-shadow: var(--shadow-half);
                                                ")}
                                                onclick={
                                                    let name = name.clone();
                                                    Callback::from(move |_| {
                                                        let name = name.clone();
                                                        wasm_bindgen_futures::spawn_local(async move {
                                                            if delete(name).await.is_ok() {
                                                                gloo::utils::window().location().reload().unwrap();
                                                            } else {
                                                                gloo::dialogs::alert("Failed to delete user.");
                                                            }
                                                        });
                                                    })
                                                }
                                            >
                                                <icons::Delete size={16} />
                                            </button>
                                        </span>
                                    </div>
                                }).collect::<Html>()
                            }

                            <button
                                class={css!("
                                    width: 64px;
                                    height: 64px;
                                    margin: 4px 16px;
                                    padding: 16px;

                                    display: flex;
                                    align-items: center;
                                    justify-content: center;

                                    background: var(--color-light-less);
                                    border-radius: 4px;
                                    box-shadow: var(--shadow-half);
                                ")}
                                onclick={Callback::from(move |_| {
                                    gloo::utils::window().location().set_href("/register").unwrap();
                                })}
                            >
                                <icons::Plus />
                            </button>
                        </div>
                    }
                }
            }
        </>
    })
}
