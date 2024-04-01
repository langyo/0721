use std::collections::HashMap;

use stylist::{css, yew::styled_component};
use yew::prelude::*;

use crate::{
    components::{icons, GlobalSkeleton},
    functions::models::user::{delete, list},
    utils::global_state::GlobalStateContext,
};
use _database::models::user::Permission;

#[styled_component]
pub fn Users() -> HtmlResult {
    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let t = global_state.language.to_config().unwrap();

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
            <GlobalSkeleton is_loading={*is_downloading} />

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
                            padding: 0 16px;

                            display: flex;
                            align-items: center;
                            justify-content: space-between;

                            background: var(--color-light-less);
                            border-radius: 4px;
                            box-shadow: var(--shadow-half);
                        ")}>
                            <span class={css!("
                                width: max-content;
                                height: 64px;

                                display: flex;
                                flex-direction: column;
                                align-items: flex-start;
                                justify-content: center;

                                font-size: 16px;
                                user-select: none;
                            ")}>
                                <p class={css!("
                                    height: 24px;
                                    line-height: 24px;
                                    font-weight: bolder;

                                    display: flex;
                                    align-items: center;
                                    justify-content: center;
                                ")}>
                                    {name.clone()}

                                    {
                                        if item.permission == Permission::Manager {
                                            html! {
                                                <span class={css!("
                                                    height: 24px;
                                                    margin-left: 8px;

                                                    line-height: 24px;
                                                    font-size: 12px;
                                                    font-style: italic;
                                                    user-select: none;
                                                ")}>
                                                    { format!("({})", t.header.manager) }
                                                </span>
                                            }
                                        } else {
                                            html! {}
                                        }
                                    }
                                </p>
                                <p class={css!("
                                    height: 24px;
                                    line-height: 24px;
                                    font-size: 12px;
                                ")}>
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
        </>
    })
}
