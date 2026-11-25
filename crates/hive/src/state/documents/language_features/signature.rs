use super::*;
use std::ops::Range as ByteRange;

/// A signature's plain label and the UTF-8 byte range to emphasize.
pub(crate) struct CallSignature {
    pub label: String,
    pub active: Option<ByteRange<usize>>,
}

impl CallSignature {
    fn from_help(help: SignatureHelp) -> Option<Self> {
        let index = help.active_signature.unwrap_or_default() as usize;
        let info = help
            .signatures
            .get(index)
            .or_else(|| help.signatures.first())?;
        let active = info
            .active_parameter
            .or(help.active_parameter)
            .unwrap_or_default() as usize;
        let range = info.parameters.as_ref().and_then(|params| {
            let parameter = params.get(active)?;
            match &parameter.label {
                ParameterLabel::LabelOffsets([start, end]) => {
                    let offset = |column: u32| {
                        let mut count = 0;
                        for (ix, c) in info.label.char_indices() {
                            if count == column {
                                return Some(ix);
                            }
                            count += c.len_utf16() as u32;
                        }
                        (count == column).then_some(info.label.len())
                    };
                    (*start < *end).then_some(offset(*start)?..offset(*end)?)
                }
                ParameterLabel::Simple(_) => {
                    let mut from = info.label.find('(').map_or(0, |ix| ix + 1);
                    for (index, parameter) in params[..=active].iter().enumerate() {
                        let ParameterLabel::Simple(label) = &parameter.label else {
                            return None;
                        };
                        let start = from + info.label.get(from..)?.find(label)?;
                        from = start + label.len();
                        if index == active {
                            return Some(start..from);
                        }
                    }
                    None
                }
            }
        });
        Some(Self {
            label: info.label.clone(),
            active: range,
        })
    }
}

impl Document {
    pub(crate) fn signature(
        document: &Entity<Self>,
        cx: &mut App,
    ) -> Task<Result<Option<CallSignature>, String>> {
        let editor = document.read(cx).editor.read(cx);
        let (text, offset) = (editor.text().clone(), editor.cursor());
        let Some((snapshot, client, text_document_position_params)) = Snapshot::capture(
            &document.downgrade(),
            &text,
            offset,
            |caps| caps.signature_help_provider.is_some(),
            cx,
        ) else {
            return Task::ready(Ok(None));
        };
        let request = client.request::<request::SignatureHelpRequest>(SignatureHelpParams {
            text_document_position_params,
            work_done_progress_params: Default::default(),
            context: None,
        });
        cx.spawn(async move |cx| {
            let result = request
                .map_err(|e| e.to_string())?
                .receive()
                .await
                .map_err(|e| e.to_string())?;
            Ok(if cx.update(|cx| snapshot.current(cx)) {
                result.and_then(CallSignature::from_help)
            } else {
                None
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CallSignature, SignatureHelp};
    #[test]
    fn highlights_the_active_overload_and_utf16_parameter() {
        let help: SignatureHelp = serde_json::from_value(serde_json::json!({
            "activeSignature": 1,
            "activeParameter": 0,
            "signatures": [
                {"label":"f()"},
                {"label":"f(😀, beta)", "activeParameter":1, "parameters":[{"label":[2,4]}, {"label":[6,10]}]}
            ]
        })).unwrap();
        let signature = CallSignature::from_help(help).unwrap();
        assert_eq!(&signature.label[signature.active.unwrap()], "beta");
    }
}
