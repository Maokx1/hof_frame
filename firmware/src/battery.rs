#[cfg(target_os = "espidf")]
use esp_idf_svc::hal::adc::{
    attenuation,
    oneshot::{
        config::{AdcChannelConfig, Calibration},
        AdcChannelDriver, AdcDriver,
    },
    Adc, ADCCH7, ADCU1,
};
#[cfg(target_os = "espidf")]
use esp_idf_svc::hal::delay::FreeRtos;
#[cfg(target_os = "espidf")]
use esp_idf_svc::hal::gpio::ADCPin;
#[cfg(target_os = "espidf")]
use esp_idf_svc::sys::EspError;

const DIVIDER_RATIO: u32 = 5;
const SAMPLE_COUNT: u32 = 16;
const EMPTY_PACK_MV: u32 = 4_400;
const FULL_PACK_MV: u32 = 5_800;
const MIN_VALID_PACK_MV: u32 = 3_500;
const MAX_VALID_PACK_MV: u32 = 6_500;

pub fn estimated_percentage(pack_mv: u32) -> Option<u8> {
    if !(MIN_VALID_PACK_MV..=MAX_VALID_PACK_MV).contains(&pack_mv) {
        return None;
    }

    let bounded_mv = pack_mv.clamp(EMPTY_PACK_MV, FULL_PACK_MV);
    Some(((bounded_mv - EMPTY_PACK_MV) * 100 / (FULL_PACK_MV - EMPTY_PACK_MV)) as u8)
}

#[cfg(target_os = "espidf")]
pub struct BatterySensor<'d> {
    channel: AdcChannelDriver<'d, ADCCH7<ADCU1>, AdcDriver<'d, ADCU1>>,
}

#[cfg(target_os = "espidf")]
impl<'d> BatterySensor<'d> {
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

#[cfg(test)]
#[path = "../tests/unit/battery.rs"]
mod tests;
