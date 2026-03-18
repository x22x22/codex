use super::*;
use pretty_assertions::assert_eq;

#[test]
fn parses_id_token_claims() {
    let jwt = "eyJhbGciOiJub25lIn0.eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20iLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9wbGFuX3R5cGUiOiJwcm8iLCJjaGF0Z3B0X3VzZXJfaWQiOiJ1c2VyLTEiLCJjaGF0Z3B0X2FjY291bnRfaWQiOiJ3cy0xIn19.c2ln";

    let claims = parse_chatgpt_jwt_claims(jwt).expect("jwt should parse");

    assert_eq!(
        claims,
        IdTokenInfo {
            email: Some("user@example.com".to_string()),
            chatgpt_plan_type: Some(PlanType::Known(KnownPlan::Pro)),
            chatgpt_user_id: Some("user-1".to_string()),
            chatgpt_account_id: Some("ws-1".to_string()),
            raw_jwt: jwt.to_string(),
        }
    );
}
