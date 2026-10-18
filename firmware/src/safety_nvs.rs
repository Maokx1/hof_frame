use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use esp_idf_svc::sys::EspError;
use firmware::safety::{SafetyError, SafetyGuard, SafetyStore};

pub struct NvsSafetyStore(EspDefaultNvs);

impl SafetyStore for NvsSafetyStore {
    type Error = EspError;

    fn read(&mut self) -> Result<Option<u8>, EspError> {
        self.0.get_u8("state_v1")
    }

    fn write(&mut self, value: u8) -> Result<(), EspError> {
        self.0.set_u8("state_v1", value)
    }
}

pub fn open(
    namespace: &str,
    brownout: bool,
) -> Result<SafetyGuard<NvsSafetyStore>, SafetyError<EspError>> {
    // Never erase a full/incompatible partition containing recovery metadata.
    let partition = EspDefaultNvsPartition::take_with(false).map_err(SafetyError::Storage)?;
    let nvs = EspDefaultNvs::new(partition, namespace, true).map_err(SafetyError::Storage)?;
    SafetyGuard::open(NvsSafetyStore(nvs), brownout)
}
