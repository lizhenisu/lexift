use crate::{
    Result,
    domain::translation::{TranslateRequest, TranslateResult},
};

pub trait TranslatorPort {
    fn translate(&self, request: TranslateRequest) -> Result<TranslateResult>;
}
