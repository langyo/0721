use yew::prelude::*;

use _database::types::response::UserInfo;

#[derive(Debug, PartialEq, Clone)]
pub struct GlobalState {
    pub token: Option<UserInfo>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum GlobalStateAction {
    SetToken(Option<UserInfo>),
}

impl Reducible for GlobalState {
    type Action = GlobalStateAction;

    fn reduce(self: std::rc::Rc<Self>, action: Self::Action) -> std::rc::Rc<Self> {
        GlobalState {
            token: match action {
                GlobalStateAction::SetToken(token) => token,
            },
        }
        .into()
    }
}

impl Default for GlobalState {
    fn default() -> Self {
        GlobalState { token: None }
    }
}

pub type GlobalStateContext = UseReducerHandle<GlobalState>;

#[derive(Properties, Debug, PartialEq)]
pub struct GlobalStateProviderProps {
    #[prop_or_default]
    pub children: Html,
}

#[function_component]
pub fn Provider(props: &GlobalStateProviderProps) -> Html {
    let state = use_reducer(|| GlobalState::default());

    html! {
        <ContextProvider<GlobalStateContext> context={state}>
            {props.children.clone()}
        </ContextProvider<GlobalStateContext>>
    }
}
