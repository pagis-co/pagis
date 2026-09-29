//! Tests for `cargo xtask emergency-numbers` (ADR-0018): the
//! generator that turns libphonenumber's metadata into the emergency
//! table `pagis-telephony` compiles in.

use xtask::emergency::{TABLE_PATH, generate_table};

const PHONE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE phoneNumberMetadata [
    <!ELEMENT phoneNumberMetadata (territories)>
]>
<phoneNumberMetadata>
  <territories>
    <territory id="CA" countryCode="1" internationalPrefix="011">
      <generalDesc><nationalNumberPattern>\d{10}</nationalNumberPattern></generalDesc>
    </territory>
    <territory id="US" countryCode="1" mainCountryForCode="true">
      <generalDesc><nationalNumberPattern>\d{10}</nationalNumberPattern></generalDesc>
    </territory>
    <territory id="BR" countryCode="55">
      <generalDesc><nationalNumberPattern>\d{10}</nationalNumberPattern></generalDesc>
    </territory>
    <territory id="GN" countryCode="224">
      <generalDesc><nationalNumberPattern>\d{10}</nationalNumberPattern></generalDesc>
    </territory>
    <territory id="001" countryCode="800">
      <generalDesc><nationalNumberPattern>\d{8}</nationalNumberPattern></generalDesc>
    </territory>
  </territories>
</phoneNumberMetadata>
"#;

const SHORT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE phoneNumberMetadata [
    <!ELEMENT phoneNumberMetadata (territories)>
]>
<phoneNumberMetadata>
  <territories>
    <territory id="BR">
      <emergency>
        <possibleLengths national="3"/>
        <exampleNumber>112</exampleNumber>
        <nationalNumberPattern>
          1(?:
            12|
            28|
            9[023]
          )|
          911
        </nationalNumberPattern>
      </emergency>
    </territory>
    <territory id="CA">
      <emergency>
        <possibleLengths national="3"/>
        <exampleNumber>112</exampleNumber>
        <nationalNumberPattern>
          112|
          911
        </nationalNumberPattern>
      </emergency>
    </territory>
    <territory id="GN">
      <shortCode>
        <possibleLengths national="3"/>
        <exampleNumber>100</exampleNumber>
        <nationalNumberPattern>1\d\d</nationalNumberPattern>
      </shortCode>
    </territory>
    <territory id="US">
      <emergency>
        <possibleLengths national="3"/>
        <exampleNumber>112</exampleNumber>
        <nationalNumberPattern>
          112|
          911
        </nationalNumberPattern>
      </emergency>
    </territory>
  </territories>
</phoneNumberMetadata>
"#;

fn table() -> String {
    generate_table("v9.0.38", PHONE_XML, SHORT_XML).unwrap()
}

#[test]
fn the_table_is_stamped_with_the_upstream_tag() {
    let table = table();

    assert!(table.contains("pub const SOURCE_VERSION: &str = \"v9.0.38\";"));
    assert!(table.contains("libphonenumber v9.0.38"));
    assert!(table.contains("Apache-2.0"));
    assert!(table.contains("Do not edit"));
}

#[test]
fn every_pattern_loses_its_whitespace() {
    let table = table();

    assert!(table.contains(
        r#"Territory { region: "BR", calling_code: 55, main_for_code: true, emergency: Some("1(?:12|28|9[023])|911") }"#
    ));
    assert!(table.contains(
        r#"Territory { region: "US", calling_code: 1, main_for_code: true, emergency: Some("112|911") }"#
    ));
}

#[test]
fn a_shared_calling_code_marks_its_main_territory_only() {
    let table = table();

    assert!(table.contains(
        r#"Territory { region: "CA", calling_code: 1, main_for_code: false, emergency: Some("112|911") }"#
    ));
    assert!(table.contains(r#"region: "US", calling_code: 1, main_for_code: true"#));
}

#[test]
fn a_territory_without_an_emergency_pattern_stays_in_the_table() {
    let table = table();

    assert!(table.contains(
        r#"Territory { region: "GN", calling_code: 224, main_for_code: true, emergency: None }"#
    ));
}

#[test]
fn non_geographic_entities_are_left_out() {
    let table = table();

    assert!(!table.contains("\"001\""));
    assert!(!table.contains("calling_code: 800"));
}

#[test]
fn the_rows_are_sorted_by_region_so_a_diff_reads() {
    let table = table();

    let br = table.find("region: \"BR\"").unwrap();
    let ca = table.find("region: \"CA\"").unwrap();
    let gn = table.find("region: \"GN\"").unwrap();
    let us = table.find("region: \"US\"").unwrap();
    assert!(br < ca && ca < gn && gn < us);
}

#[test]
fn a_short_number_territory_the_phone_metadata_lacks_is_an_error() {
    let short = SHORT_XML.replace(r#"<territory id="CA">"#, r#"<territory id="ZZ">"#);

    let error = generate_table("v9.0.38", PHONE_XML, &short).unwrap_err();

    assert!(error.to_string().contains("ZZ"), "{error}");
}

#[test]
fn the_table_lands_in_the_telephony_crate() {
    assert_eq!(TABLE_PATH, "crates/pagis-telephony/src/emergency/table.rs");
}
