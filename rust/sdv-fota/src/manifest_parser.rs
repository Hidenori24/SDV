//! Heap-free parser for the fixed v1 signed manifest schema.

use crate::{
    Ecu, FixedString, FixedU32List, Image, Manifest, Requirements, UpdatePolicy, ValidationError,
    MAX_CONFIG_SCHEMA_VERSIONS,
};

type ParseResult<T> = Result<T, ValidationError>;

pub(crate) fn parse_manifest(bytes: &[u8]) -> ParseResult<Manifest> {
    let mut parser = Parser { bytes, offset: 0 };
    parser.whitespace();
    parser.expect(b'{')?;
    let mut seen = 0u8;
    let mut schema = 0;
    let mut image = None;
    let mut requires = None;
    let mut policy = None;
    if !parser.consume(b'}') {
        loop {
            let key = parser.string::<32>()?;
            parser.expect(b':')?;
            match key.as_str() {
                "manifest_schema" => {
                    mark(&mut seen, 1)?;
                    schema = parser.u32()?;
                }
                "image" => {
                    mark(&mut seen, 2)?;
                    image = Some(parser.image()?);
                }
                "requires" => {
                    mark(&mut seen, 4)?;
                    requires = Some(parser.requirements()?);
                }
                "policy" => {
                    mark(&mut seen, 8)?;
                    policy = Some(parser.policy()?);
                }
                _ => return Err(ValidationError::InvalidManifest),
            }
            if parser.consume(b'}') {
                break;
            }
            parser.expect(b',')?;
        }
    }
    parser.finish()?;
    if seen != 0x0f {
        return Err(ValidationError::InvalidManifest);
    }
    Ok(Manifest {
        manifest_schema: schema,
        image: image.ok_or(ValidationError::InvalidManifest)?,
        requires: requires.ok_or(ValidationError::InvalidManifest)?,
        policy: policy.ok_or(ValidationError::InvalidManifest)?,
    })
}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Parser<'a> {
    fn image(&mut self) -> ParseResult<Image> {
        self.object_start()?;
        let mut seen = 0u8;
        let mut target_ecu = None;
        let mut swc = None;
        let mut version = None;
        let mut security_version = 0;
        let mut size_bytes = 0;
        let mut sha256 = None;
        self.fields(|parser, key| match key {
            "target_ecu" => {
                mark(&mut seen, 1)?;
                target_ecu = Some(match parser.string::<16>()?.as_str() {
                    "engine" => Ecu::Engine,
                    "brake" => Ecu::Brake,
                    "steering" => Ecu::Steering,
                    _ => return Err(ValidationError::InvalidManifest),
                });
                Ok(())
            }
            "swc" => {
                mark(&mut seen, 2)?;
                swc = Some(parser.string::<64>()?);
                Ok(())
            }
            "version" => {
                mark(&mut seen, 4)?;
                version = Some(parser.string::<32>()?);
                Ok(())
            }
            "security_version" => {
                mark(&mut seen, 8)?;
                security_version = parser.u64()?;
                Ok(())
            }
            "size_bytes" => {
                mark(&mut seen, 16)?;
                size_bytes = parser.u64()?;
                Ok(())
            }
            "sha256" => {
                mark(&mut seen, 32)?;
                sha256 = Some(parser.string::<64>()?);
                Ok(())
            }
            _ => Err(ValidationError::InvalidManifest),
        })?;
        if seen != 0x3f {
            return Err(ValidationError::InvalidManifest);
        }
        Ok(Image {
            target_ecu: target_ecu.ok_or(ValidationError::InvalidManifest)?,
            swc: swc.ok_or(ValidationError::InvalidManifest)?,
            version: version.ok_or(ValidationError::InvalidManifest)?,
            security_version,
            size_bytes,
            sha256: sha256.ok_or(ValidationError::InvalidManifest)?,
        })
    }

    fn requirements(&mut self) -> ParseResult<Requirements> {
        self.object_start()?;
        let mut seen = 0u8;
        let mut signal_if_version = 0;
        let mut config_schema = None;
        self.fields(|parser, key| match key {
            "signal_if_version" => {
                mark(&mut seen, 1)?;
                signal_if_version = parser.u32()?;
                Ok(())
            }
            "config_schema" => {
                mark(&mut seen, 2)?;
                config_schema = Some(parser.config_schema()?);
                Ok(())
            }
            _ => Err(ValidationError::InvalidManifest),
        })?;
        if seen != 3 {
            return Err(ValidationError::InvalidManifest);
        }
        Ok(Requirements {
            signal_if_version,
            config_schema: config_schema.ok_or(ValidationError::InvalidManifest)?,
        })
    }

    fn policy(&mut self) -> ParseResult<UpdatePolicy> {
        self.object_start()?;
        let mut seen = 0u8;
        let mut allow_downgrade = false;
        let mut trial_ms = 0;
        self.fields(|parser, key| match key {
            "allow_downgrade" => {
                mark(&mut seen, 1)?;
                allow_downgrade = parser.boolean()?;
                Ok(())
            }
            "trial_ms" => {
                mark(&mut seen, 2)?;
                trial_ms = parser.u32()?;
                Ok(())
            }
            _ => Err(ValidationError::InvalidManifest),
        })?;
        if seen != 3 {
            return Err(ValidationError::InvalidManifest);
        }
        Ok(UpdatePolicy {
            allow_downgrade,
            trial_ms,
        })
    }

    fn config_schema(&mut self) -> ParseResult<FixedU32List<MAX_CONFIG_SCHEMA_VERSIONS>> {
        self.whitespace();
        self.expect(b'[')?;
        let mut versions = FixedU32List::new();
        self.whitespace();
        if self.consume(b']') {
            return Ok(versions);
        }
        loop {
            if !versions.try_push(self.u32()?) {
                return Err(ValidationError::InvalidManifest);
            }
            if self.consume(b']') {
                return Ok(versions);
            }
            self.expect(b',')?;
        }
    }

    fn fields<F>(&mut self, mut field: F) -> ParseResult<()>
    where
        F: FnMut(&mut Self, &str) -> ParseResult<()>,
    {
        self.whitespace();
        if self.consume(b'}') {
            return Ok(());
        }
        loop {
            let key = self.string::<32>()?;
            self.expect(b':')?;
            field(self, key.as_str())?;
            if self.consume(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn object_start(&mut self) -> ParseResult<()> {
        self.whitespace();
        self.expect(b'{')
    }

    fn string<const N: usize>(&mut self) -> ParseResult<FixedString<N>> {
        self.whitespace();
        self.expect(b'"')?;
        let mut decoded = [0u8; N];
        let mut len = 0;
        loop {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or(ValidationError::InvalidManifest)?;
            match byte {
                b'"' => {
                    self.offset += 1;
                    return FixedString::try_from_bytes(&decoded[..len])
                        .ok_or(ValidationError::InvalidManifest);
                }
                b'\\' => {
                    self.offset += 1;
                    self.parse_escape(&mut decoded, &mut len)?;
                }
                0..=0x1f => return Err(ValidationError::InvalidManifest),
                _ => {
                    append(&mut decoded, &mut len, &[byte])?;
                    self.offset += 1;
                }
            }
        }
    }

    fn parse_escape<const N: usize>(
        &mut self,
        decoded: &mut [u8; N],
        len: &mut usize,
    ) -> ParseResult<()> {
        let escaped = *self
            .bytes
            .get(self.offset)
            .ok_or(ValidationError::InvalidManifest)?;
        self.offset += 1;
        let byte = match escaped {
            b'"' => b'"',
            b'\\' => b'\\',
            b'/' => b'/',
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'u' => {
                let first = self.hex_quad()?;
                let scalar = if (0xd800..=0xdbff).contains(&first) {
                    if self.bytes.get(self.offset..self.offset + 2) != Some(&b"\\u"[..]) {
                        return Err(ValidationError::InvalidManifest);
                    }
                    self.offset += 2;
                    let second = self.hex_quad()?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(ValidationError::InvalidManifest);
                    }
                    0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
                } else if (0xdc00..=0xdfff).contains(&first) {
                    return Err(ValidationError::InvalidManifest);
                } else {
                    u32::from(first)
                };
                let character = char::from_u32(scalar).ok_or(ValidationError::InvalidManifest)?;
                let mut utf8 = [0u8; 4];
                append(decoded, len, character.encode_utf8(&mut utf8).as_bytes())?;
                return Ok(());
            }
            _ => return Err(ValidationError::InvalidManifest),
        };
        append(decoded, len, &[byte])
    }

    fn hex_quad(&mut self) -> ParseResult<u16> {
        let mut value = 0u16;
        for _ in 0..4 {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or(ValidationError::InvalidManifest)?;
            self.offset += 1;
            let nibble = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(ValidationError::InvalidManifest),
            };
            value = (value << 4) | u16::from(nibble);
        }
        Ok(value)
    }

    fn u32(&mut self) -> ParseResult<u32> {
        u32::try_from(self.u64()?).map_err(|_| ValidationError::InvalidManifest)
    }

    fn u64(&mut self) -> ParseResult<u64> {
        self.whitespace();
        let start = self.offset;
        let mut value = 0u64;
        while let Some(byte @ b'0'..=b'9') = self.bytes.get(self.offset).copied() {
            value = value
                .checked_mul(10)
                .and_then(|number| number.checked_add(u64::from(byte - b'0')))
                .ok_or(ValidationError::InvalidManifest)?;
            self.offset += 1;
        }
        if start == self.offset || (self.offset - start > 1 && self.bytes[start] == b'0') {
            return Err(ValidationError::InvalidManifest);
        }
        Ok(value)
    }

    fn boolean(&mut self) -> ParseResult<bool> {
        self.whitespace();
        if self.take_literal(b"true") {
            Ok(true)
        } else if self.take_literal(b"false") {
            Ok(false)
        } else {
            Err(ValidationError::InvalidManifest)
        }
    }

    fn finish(&mut self) -> ParseResult<()> {
        self.whitespace();
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(ValidationError::InvalidManifest)
        }
    }

    fn whitespace(&mut self) {
        while matches!(
            self.bytes.get(self.offset),
            Some(b' ' | b'\n' | b'\r' | b'\t')
        ) {
            self.offset += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> ParseResult<()> {
        self.whitespace();
        if self.consume(byte) {
            Ok(())
        } else {
            Err(ValidationError::InvalidManifest)
        }
    }

    fn consume(&mut self, byte: u8) -> bool {
        self.whitespace();
        if self.bytes.get(self.offset) == Some(&byte) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn take_literal(&mut self, value: &[u8]) -> bool {
        if self.bytes.get(self.offset..self.offset + value.len()) == Some(value) {
            self.offset += value.len();
            true
        } else {
            false
        }
    }
}

fn append<const N: usize>(buffer: &mut [u8; N], len: &mut usize, bytes: &[u8]) -> ParseResult<()> {
    let end = len
        .checked_add(bytes.len())
        .filter(|end| *end <= N)
        .ok_or(ValidationError::InvalidManifest)?;
    buffer[*len..end].copy_from_slice(bytes);
    *len = end;
    Ok(())
}

fn mark(seen: &mut u8, bit: u8) -> ParseResult<()> {
    if *seen & bit != 0 {
        return Err(ValidationError::InvalidManifest);
    }
    *seen |= bit;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_manifest;

    const MANIFEST: &[u8] = br#"{"manifest_schema":1,"image":{"target_ecu":"engine","swc":"EngineSWC","version":"1.2.3","security_version":2,"size_bytes":4,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"},"requires":{"signal_if_version":1,"config_schema":[1,2]},"policy":{"allow_downgrade":false,"trial_ms":5000}}"#;

    #[test]
    fn parses_valid_manifest_without_dynamic_fields() {
        let manifest = parse_manifest(MANIFEST).unwrap();
        assert_eq!(manifest.image.version, "1.2.3");
        assert_eq!(&*manifest.requires.config_schema, &[1, 2]);
    }

    #[test]
    fn accepts_json_whitespace_and_field_reordering() {
        let reordered = br#"{
          "policy": {"trial_ms": 5000, "allow_downgrade": false},
          "requires": {"config_schema": [1, 2], "signal_if_version": 1},
          "image": {"sha256": "0000000000000000000000000000000000000000000000000000000000000000", "size_bytes": 4, "security_version": 2, "version": "1.2.3", "swc": "EngineSWC", "target_ecu": "engine"},
          "manifest_schema": 1
        }"#;
        let manifest = parse_manifest(reordered).unwrap();
        assert_eq!(manifest.image.version, "1.2.3");
    }

    #[test]
    fn decodes_escaped_strings_and_rejects_over_capacity_values() {
        let source = core::str::from_utf8(MANIFEST).unwrap();
        let escaped = source.replace("1.2.3", r"1.2\u002e3");
        assert_eq!(
            parse_manifest(escaped.as_bytes()).unwrap().image.version,
            "1.2.3"
        );

        let long_version = source.replace("1.2.3", "123456789012345678901234567890123");
        assert!(parse_manifest(long_version.as_bytes()).is_err());

        let long_schema = source.replace("[1,2]", "[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17]");
        assert!(parse_manifest(long_schema.as_bytes()).is_err());
    }

    #[test]
    fn decodes_unicode_surrogate_pairs_and_rejects_unpaired_surrogates() {
        let source = core::str::from_utf8(MANIFEST).unwrap();
        let paired = source.replace("EngineSWC", r"Engine\uD83D\uDE97SWC");
        let manifest = parse_manifest(paired.as_bytes()).unwrap();
        assert_eq!(manifest.image.swc.as_str(), "Engine🚗SWC");

        let unpaired = source.replace("EngineSWC", r"Engine\uD800SWC");
        assert!(parse_manifest(unpaired.as_bytes()).is_err());
    }

    #[test]
    fn rejects_unknown_duplicate_and_trailing_fields() {
        let source = core::str::from_utf8(MANIFEST).unwrap();
        let unknown = source.replace("\"manifest_schema\":1", "\"manifest_schema\":1,\"other\":0");
        assert!(parse_manifest(unknown.as_bytes()).is_err());
        let duplicate = source.replace(
            "\"manifest_schema\":1",
            "\"manifest_schema\":1,\"manifest_schema\":1",
        );
        assert!(parse_manifest(duplicate.as_bytes()).is_err());
        let mut trailing = MANIFEST.to_vec();
        trailing.push(b' ');
        trailing.push(b'x');
        assert!(parse_manifest(&trailing).is_err());
    }
}
