use std::time::Duration;

pub async fn mail_retry(
    js: &async_nats::jetstream::Context,
    subject: String,
    headers: async_nats::HeaderMap,
    body: String,
) -> Result<async_nats::jetstream::publish::PublishAck, async_nats::jetstream::context::PublishError>
{
    retry_mail(
        js,
        subject,
        headers,
        body,
        vega_registry::BROKER_WAIT,
        vega_registry::BROKER_WAIT / 3,
    )
    .await
}

pub(crate) async fn retry_mail(
    js: &async_nats::jetstream::Context,
    subject: String,
    headers: async_nats::HeaderMap,
    body: String,
    budget: Duration,
    attempt: Duration,
) -> Result<async_nats::jetstream::publish::PublishAck, async_nats::jetstream::context::PublishError>
{
    use async_nats::jetstream::context::{PublishError, PublishErrorKind as K};
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let until = deadline.min(tokio::time::Instant::now() + attempt);
        let result = tokio::time::timeout_at(until, async {
            mail(js, subject.clone(), headers.clone(), body.clone())
                .await?
                .await
        })
        .await
        .unwrap_or_else(|_| Err(PublishError::new(K::TimedOut)));
        match result {
            Err(e)
                if matches!(
                    e.kind(),
                    K::TimedOut | K::Other | K::BrokenPipe | K::MaxAckPending
                ) && tokio::time::Instant::now() + Duration::from_millis(250) < deadline =>
            {
                eprintln!("post: retrying publish after {e} (same message id)");
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            result => return result,
        }
    }
}

#[cfg(all(unix, not(feature = "client-only")))]
use crate::history::mail;
#[cfg(any(windows, feature = "client-only"))]
async fn mail(
    js: &async_nats::jetstream::Context,
    subject: String,
    headers: async_nats::HeaderMap,
    body: String,
) -> Result<
    async_nats::jetstream::context::PublishAckFuture,
    async_nats::jetstream::context::PublishError,
> {
    js.publish_with_headers(subject, headers, body.into()).await
}
