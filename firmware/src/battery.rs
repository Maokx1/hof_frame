const DIVIDER_RATIO: u32 = 5;
const SAMPLE_COUNT: u32 = 16;
const EMPTY_PACK_MV: u32 = 4_400;
const FULL_PACK_MV: u32 = 5_800;
const MIN_VALID_PACK_MV: u32 = 3_500;
const MAX_VALID_PACK_MV: u32 = 6_500;

/// Estimate NiMH charge for display only; reject implausible pack voltages.
pub fn estimated_percentage(pack_mv: u32) -> Option<u8> {
    if !(MIN_VALID_PACK_MV..=MAX_VALID_PACK_MV).contains(&pack_mv) {
        return None;
    }

    let bounded_mv = pack_mv.clamp(EMPTY_PACK_MV, FULL_PACK_MV);
    Some(((bounded_mv - EMPTY_PACK_MV) * 100 / (FULL_PACK_MV - EMPTY_PACK_MV)) as u8)
}

espidf_items! {
    use esp_idf_svc::hal::adc::{
        attenuation,
        oneshot::{
            config::{AdcChannelConfig, Calibration},
            AdcChannelDriver, AdcDriver,
        },
        Adc, ADCCH7, ADCU1,
    };
    use esp_idf_svc::hal::delay::FreeRtos;
    use esp_idf_svc::hal::gpio::ADCPin;
    use esp_idf_svc::sys::EspError;

    pub struct BatterySensor<'d> {
        channel: AdcChannelDriver<'d, ADCCH7<ADCU1>, AdcDriver<'d, ADCU1>>,
    }

    impl<'d> BatterySensor<'d> {
        /// Configure the external pack-divider ADC with curve calibration.
        pub fn new<ADC, PIN>(adc: ADC, pin: PIN) -> Result<Self, EspError>
        where
            ADC: Adc<AdcUnit = ADCU1> + 'd,
            PIN: ADCPin<AdcChannel = ADCCH7<ADCU1>> + 'd,
        {
            let driver = AdcDriver::new(adc)?;
            let config = AdcChannelConfig {
                attenuation: attenuation::DB_12,
                calibration: Calibration::Curve,
                ..Default::default()
            };
            let channel = AdcChannelDriver::new(driver, pin, &config)?;
            Ok(Self { channel })
        }

        /// Configure an optional display sensor, explicitly logging setup failures.
        pub fn try_new<ADC, PIN>(adc: ADC, pin: PIN) -> Option<Self>
        where
            ADC: Adc<AdcUnit = ADCU1> + 'd,
            PIN: ADCPin<AdcChannel = ADCCH7<ADCU1>> + 'd,
        {
            match Self::new(adc, pin) {
                Ok(sensor) => Some(sensor),
                Err(error) => {
                    log::warn!("battery ADC setup failed; website will show ?%: {error:?}");
                    None
                }
            }
        }

        /// Read a display estimate, logging invalid readings or unavailable ADC data.
        pub fn read_optional(sensor: Option<&mut Self>) -> Option<u8> {
            match sensor?.read_percentage() {
                Ok(Some(percentage)) => Some(percentage),
                Ok(None) => {
                    log::warn!(
                        "battery reading outside the expected pack-voltage range; website will show ?%"
                    );
                    None
                }
                Err(error) => {
                    log::warn!("battery ADC read failed; website will show ?%: {error:?}");
                    None
                }
            }
        }

        /// Average divider samples and convert them into a display-only estimate.
        pub fn read_percentage(&mut self) -> Result<Option<u8>, EspError> {
            let mut sum_mv = 0_u32;
            for sample in 0..SAMPLE_COUNT {
                sum_mv += u32::from(self.channel.read()?);
                if sample + 1 < SAMPLE_COUNT {
                    FreeRtos::delay_ms(2);
                }
            }

            let pin_mv = sum_mv / SAMPLE_COUNT;
            Ok(estimated_percentage(pin_mv * DIVIDER_RATIO))
        }
    }
}
