use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct LocaleConfig {
    pub country: String,
    pub region: Option<String>,
    pub timezone: String,
    pub language: String,
    pub cultural: CulturalConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CulturalConfig {
    pub week_starts_on: String,
    pub meal_pattern: MealPattern,
    pub working_days: Vec<String>,
    pub currency: String,
    pub temperature_unit: TempUnit,
    pub distance_unit: DistanceUnit,
}

#[derive(Debug, Clone, Deserialize)]
pub enum MealPattern {
    ThreeMeals,
    FourMeals,
    Custom(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
pub enum TempUnit {
    Celsius,
    Fahrenheit,
}

#[derive(Debug, Clone, Deserialize)]
pub enum DistanceUnit {
    Kilometers,
    Miles,
}
