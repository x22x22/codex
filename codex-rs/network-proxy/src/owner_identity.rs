use rama_core::extensions::Extensions;
use rama_core::extensions::ExtensionsRef;
use rama_http::headers::authorization::AuthoritySync;
use rama_net::user::Basic;
use rama_net::user::UserId;
use rama_net::user::authority::AuthorizeResult;
use rama_net::user::authority::Authorizer;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProxyOwnerAuthorizer;

impl AuthoritySync<Basic, ()> for ProxyOwnerAuthorizer {
    fn authorized(&self, ext: &mut Extensions, credentials: &Basic) -> bool {
        ext.insert(UserId::Username(credentials.username().to_string()));
        true
    }
}

impl Authorizer<Basic> for ProxyOwnerAuthorizer {
    type Error = std::convert::Infallible;

    async fn authorize(&self, credentials: Basic) -> AuthorizeResult<Basic, Self::Error> {
        let mut extensions = Extensions::new();
        extensions.insert(UserId::Username(credentials.username().to_string()));
        AuthorizeResult {
            credentials,
            result: Ok(Some(extensions)),
        }
    }
}

pub(crate) fn network_owner_id<T: ExtensionsRef>(input: &T) -> Option<String> {
    match input.extensions().get::<UserId>() {
        Some(UserId::Username(username)) => Some(username.clone()),
        Some(UserId::Token(token)) => String::from_utf8(token.clone()).ok(),
        Some(UserId::Anonymous) | None => None,
    }
}
