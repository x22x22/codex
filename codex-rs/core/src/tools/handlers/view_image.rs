use async_trait::async_trait;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::local_image_content_items_from_bytes_with_label_number;
use codex_protocol::openai_models::InputModality;
use codex_utils_image::PromptImageMode;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::original_image_detail::can_request_original_image_detail;
use crate::protocol::EventMsg;
use crate::protocol::ViewImageToolCallEvent;
use crate::sandboxed_fs;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ViewImageHandler;

const VIEW_IMAGE_UNSUPPORTED_MESSAGE: &str =
    "view_image is not allowed because you do not support image inputs";

#[derive(Deserialize)]
struct ViewImageArgs {
    path: String,
    detail: Option<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ViewImageDetail {
    Original,
}

#[async_trait]
impl ToolHandler for ViewImageHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        if !invocation
            .turn
            .model_info
            .input_modalities
            .contains(&InputModality::Image)
        {
            return Err(FunctionCallError::RespondToModel(
                VIEW_IMAGE_UNSUPPORTED_MESSAGE.to_string(),
            ));
        }

        let ToolInvocation {
            session,
            turn,
            payload,
            call_id,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "view_image handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ViewImageArgs = parse_arguments(&arguments)?;
        // `view_image` accepts only its documented detail values: omit
        // `detail` for the default path or set it to `original`.
        // Other string values remain invalid rather than being silently
        // reinterpreted.
        let detail = match args.detail.as_deref() {
            None => None,
            Some("original") => Some(ViewImageDetail::Original),
            Some(detail) => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "view_image.detail only supports `original`; omit `detail` for default resized behavior, got `{detail}`"
                )));
            }
        };

        let abs_path = turn.resolve_path(Some(args.path));
        let event_path = abs_path.clone();

        let can_request_original_detail =
            can_request_original_image_detail(turn.features.get(), &turn.model_info);
        let use_original_detail =
            can_request_original_detail && matches!(detail, Some(ViewImageDetail::Original));
        let image_mode = if use_original_detail {
            PromptImageMode::Original
        } else {
            PromptImageMode::ResizeToFit
        };
        let image_detail = use_original_detail.then_some(ImageDetail::Original);
        let image_bytes = sandboxed_fs::read_bytes(&session, &turn, &abs_path)
            .await
            .map_err(|error| {
                FunctionCallError::RespondToModel(render_view_image_read_error(&abs_path, &error))
            })?;

        let content = local_image_content_items_from_bytes_with_label_number(
            &abs_path,
            image_bytes,
            /*label_number*/ None,
            image_mode,
        )
        .into_iter()
        .map(|item| match item {
            ContentItem::InputText { text } => FunctionCallOutputContentItem::InputText { text },
            ContentItem::InputImage { image_url } => FunctionCallOutputContentItem::InputImage {
                image_url,
                detail: image_detail,
            },
            ContentItem::OutputText { text } => FunctionCallOutputContentItem::InputText { text },
        })
        .collect();

        session
            .send_event(
                turn.as_ref(),
                EventMsg::ViewImageToolCall(ViewImageToolCallEvent {
                    call_id,
                    path: event_path,
                }),
            )
            .await;

        Ok(FunctionToolOutput::from_content(content, Some(true)))
    }
}

fn render_view_image_read_error(
    path: &std::path::Path,
    error: &sandboxed_fs::SandboxedFsError,
) -> String {
    let operation_message = error
        .operation_error_message()
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string());
    match error.operation_error_kind() {
        Some(codex_fs_ops::FsErrorKind::IsADirectory) => {
            format!("image path `{}` is not a file", path.display())
        }
        Some(codex_fs_ops::FsErrorKind::NotFound) => {
            format!(
                "unable to locate image at `{}`: {operation_message}",
                path.display()
            )
        }
        Some(_) | None => {
            format!(
                "unable to read image at `{}`: {operation_message}",
                path.display()
            )
        }
    }
}
