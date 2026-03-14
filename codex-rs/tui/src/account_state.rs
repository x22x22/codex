use crate::bottom_pane::FeedbackAudience;
use codex_app_server_protocol::Account;
use codex_protocol::account::PlanType;

pub(crate) fn account_is_chatgpt(account: Option<&Account>) -> bool {
    matches!(account, Some(Account::Chatgpt { .. }))
}

pub(crate) fn account_plan_type(account: Option<&Account>) -> Option<PlanType> {
    match account {
        Some(Account::Chatgpt { plan_type, .. }) => Some(*plan_type),
        Some(Account::ApiKey {}) | None => None,
    }
}

pub(crate) fn feedback_audience_from_account(account: Option<&Account>) -> FeedbackAudience {
    let is_openai_employee = matches!(
        account,
        Some(Account::Chatgpt { email, .. }) if email.ends_with("@openai.com")
    );
    if is_openai_employee {
        FeedbackAudience::OpenAiEmployee
    } else {
        FeedbackAudience::External
    }
}
