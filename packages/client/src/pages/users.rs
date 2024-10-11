use stylist::{css, yew::styled_component};
use yew::prelude::*;

use crate::components::{icons, GlobalSkeleton};
use _functions::{
    models::user::{count, delete, list},
    utils::global_state::GlobalStateContext,
};
use _types::request::Permission;

#[styled_component]
pub fn Users() -> HtmlResult {
    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let t = global_state.language.to_config().unwrap();

    let is_downloading = use_state(|| true);
    let user_count = use_state(|| 0);
    let user_list = use_state(Vec::new);
    use_effect_with((), {
        let is_downloading = is_downloading.clone();
        let user_count = user_count.clone();
        let user_list = user_list.clone();
        |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let user_count = user_count.clone();
                let user_list = user_list.clone();

                user_count.set(count().await.unwrap());
                user_list.set(list(None, None).await.unwrap());

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
                    user_list.iter().map(|item| html! {
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
                                    {item.name.clone()}

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
                                        let email = item.email.clone();
                                        Callback::from(move |_| {
                                            let email = email.clone();
                                            wasm_bindgen_futures::spawn_local(async move {
                                                if delete(email).await.is_ok() {
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
                    class={classes!(css!("
                        width: 128px;
                        height: 32px;
                        margin: 16px;

                        font-size: 16px;
                        font-weight: bolder;
                        text-align: center;
                    "), {
                        if (*user_count) <= (*user_list).len() {
                            css!("
                                background: var(--color-dark-less);
                                pointer-events: none;
                            ")
                        } else {
                            css!("")
                        }
                    })}
                    onclick={
                        let user_count = user_count.clone();
                        let user_list = user_list.clone();

                        Callback::from(move |_| {
                            let user_count = user_count.clone();
                            let user_list = user_list.clone();

                            wasm_bindgen_futures::spawn_local(async move {
                                let user_count = user_count.clone();
                                let user_list = user_list.clone();

                                let current_count_raw = count().await.unwrap();
                                let current_list_raw = (*user_list).clone();

                                user_count.set(current_count_raw);
                                if current_count_raw > current_list_raw.len() {
                                    user_list.set(list(
                                        Some(current_list_raw.len()),
                                        Some(100),
                                    ).await.unwrap());
                                }
                            });
                        })
                    }
                >
                    { t.images.load_more }
                </button>

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
