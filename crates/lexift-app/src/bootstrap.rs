use crate::{lifecycle, wiring::AppServices};

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    lexift_observability::init();
    lifecycle::on_start();

    let services = AppServices::new()?;
    let result = lexift_ui::run(&services.state).map_err(Into::into);

    lifecycle::on_exit();
    result
}
