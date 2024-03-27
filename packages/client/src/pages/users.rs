use std::collections::HashMap;

use stylist::{css, yew::styled_component};
use yew::prelude::*;

use crate::{components::icons, functions::models::user::list};

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
                            padding: 64px;

                            display: flex;
                            flex-direction: column;
                            align-items: flex-start;
                            justify-content: flex-start;
                        ")}>
                            {
                                user_list.iter().map(|(name, item)| html! {
                                    <div class={css!("
                                        width: calc(100% - 16px * 2);
                                        height: 64px;
                                        margin: 16px;
                                        padding: 16px;

                                        display: flex;
                                        align-items: center;
                                        justify-content: space-between;
                                    ")}>
                                        <p>
                                            {name.clone()}
                                        </p>
                                        <p>
                                            {item.email.clone()}
                                        </p>
                                    </div>
                                }).collect::<Html>()
                            }
                        </div>
                    }
                }
            }
        </>
    })
}
