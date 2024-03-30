use wasm_bindgen::JsCast as _;
use web_sys::HtmlInputElement;

use stylist::{css, yew::styled_component};
use yew::prelude::*;

use crate::{
    components::icons,
    functions::models::config::{get as get_config, set as set_config},
    utils::global_state::GlobalStateContext,
};
use _database::types::config::{load_config, Config as Model};

#[styled_component]
pub fn ConfigPage() -> HtmlResult {
    let global_config = use_prepared_state!((), async move |_| -> Option<Model> {
        if let Ok(ret) = load_config() {
            return Some(ret);
        }
        None
    })?
    .unwrap();
    let global_config = use_state(|| (*global_config).clone().unwrap_or_default());

    let global_state = use_context::<GlobalStateContext>().expect("Global state not found");

    let t = global_state.language.to_config().unwrap();

    Ok(html! {
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
            <div class={css!("
                width: 80%;
                height: 64px;
                margin: 16px;
                padding: 0 32px;

                display: flex;
                flex-direction: column;
                align-items: center;
                justify-content: center;

                background: var(--color-light-less);
                border-radius: 4px;
                box-shadow: var(--shadow-half);
            ")}>
                {
                    [
                        (
                            global_config.portal.title_suffix.clone(),
                            t.config.portal.title_suffix.clone(),
                            Callback::from(move |e: InputEvent| {
                                let target = e.target();
                                let input = target.and_then(|target| target.dyn_into::<HtmlInputElement>().ok());

                                if let Some(input) = input {
                                    log::warn!("{}", input.value());
                                }
                            }),
                        ),
                    ].iter().map(|(value, name, callback)| html! {
                        <div class={css!("
                            width: 100%;
                            height: 64px;
                            margin: 16px 0;

                            display: flex;
                            align-items: center;
                            justify-content: space-between;
                        ")}>
                            <span class={css!("
                                width: 20%;
                                min-width: 128px;
                                height: 32px;

                                font-size: 16px;
                                line-height: 32px;
                                user-select: none;
                            ")}>
                                {name.to_string()}
                            </span>
                            <input
                                class={css!("
                                    width: 80%;
                                    height: 32px;
                                    padding: 0 16px;

                                    border: none;
                                    outline: 1px solid var(--color-light-half);

                                    font-size: 16px;
                                    line-height: 32px;
                                    user-select: none;

                                    background: var(--color-dark-less);
                                    border-radius: 4px;
                                    box-shadow: var(--shadow-half);
                                ")}
                                type={"text"}
                                value={value.clone()}
                                oninput={callback.clone()}
                            />
                        </div>
                    }).collect::<Html>()
                }
            </div>
        </div>
    })
}
