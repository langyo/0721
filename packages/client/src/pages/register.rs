use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

use stylist::css;
use yew::prelude::*;

#[function_component]
pub fn Register() -> Html {
    let name_raw: UseStateHandle<String> = use_state(|| "".to_string());
    let email_raw = use_state(|| "".to_string());
    let phone_raw = use_state(|| "".to_string());

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

    html! {
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

                display: flex;
                flex-direction: column;
                align-items: center;
                justify-content: center;
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
                    { "来宾登记" }
                </h1>

                <input
                    class={input_style.clone()}
                    type="text"
                    placeholder="联系人姓名"
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
                    placeholder="联系邮箱"
                    value={(*email_raw).clone()}
                    oninput={{
                        let email_raw = email_raw.clone();
                        Callback::from(move |e: InputEvent| {
                            let target = e.target();
                            let input = target.and_then(|target| target.dyn_into::<HtmlInputElement>().ok());

                            if let Some(input) = input {
                                let value = input.value();
                                let value = value.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '@' || *c == '.').collect::<String>();

                                email_raw.set(value);
                            }
                        })
                    }}
                />
                <input
                    class={input_style.clone()}
                    type="phone"
                    placeholder="联系电话"
                    value={(*phone_raw).clone()}
                    oninput={{
                        let phone_raw = phone_raw.clone();
                        Callback::from(move |e: InputEvent| {
                            let target = e.target();
                            let input = target.and_then(|target| target.dyn_into::<HtmlInputElement>().ok());

                            if let Some(input) = input {
                                let value = input.value();
                                let value = value.chars().filter(|c| c.is_digit(10) || *c == '+').collect::<String>();

                                phone_raw.set(value);
                            }
                        })
                    }}
                />

                <button class={css!("
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
                ")}>
                    { "注册" }
                </button>
            </div>
        </div>
    }
}
