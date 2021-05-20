use crate::{
    header::HeaderValue,
    sign::{encode_bytes_with_hex, sha256_digest},
    Config, Error, SignableBody, SignedBodyHeaderType, SigningSettings, UriEncoding, DATE_FORMAT,
    HMAC_256,
};
use chrono::{format::ParseError, Date, DateTime, NaiveDate, NaiveDateTime, Utc};
use http::{header::HeaderName, HeaderMap, Method, Request};
use serde_urlencoded as qs;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    convert::{AsRef, TryFrom},
    fmt,
};

pub(crate) trait AsSigV4 {
    fn fmt(&self) -> String;
}

#[derive(Default, Debug, PartialEq)]
pub(crate) struct CanonicalRequest {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) params: String,
    pub(crate) headers: HeaderMap,
    pub(crate) signed_headers: SignedHeaders,
    pub(crate) payload_hash: String,
}

pub(crate) struct AddedHeaders {
    pub x_amz_date: String,
    pub x_amz_content_256: Option<String>,
    pub x_amz_security_token: Option<String>,
}

/*
pub (crate) fn add_signed_headers(headers: &mut HeaderMap, body: SignableBody, config: &Config) -> AddedHeaders {
        let x_amz_date = HeaderName::from_static("x-amz-date");
        let date = DateTime::<Utc>::from(config.date);
        let date_header = HeaderValue::try_from(date.fmt_aws()).expect("date is valid header value");
        headers.insert(
            x_amz_date,
            date_header.clone(),
        );

        let mut x_amz_content_256 = None;

        if config.settings.signed_body_header == SignedBodyHeaderType::XAmzSha256 {
            let digest = sha256_digest(body);
            let content_header =

        }


        AddedHeaders {
            x_amz_date: date_header,
            x_amz_content_256: None,
            x_amz_security_token: None,
        }
}*/

impl CanonicalRequest {
    pub(crate) fn from<B>(
        req: &Request<B>,
        body: SignableBody,
        settings: &SigningSettings,
        date: DateTime<Utc>,
        security_token: Option<&str>,
    ) -> Result<(CanonicalRequest, AddedHeaders), Error> {
        let path = req.uri().path();
        let path = match settings.uri_encoding {
            // The string is already URI encoded, we don't need to encode everything again, just `%`
            UriEncoding::Double => path.replace('%', "%25"),
            UriEncoding::Single => path.to_string(),
        };
        let mut creq = CanonicalRequest {
            method: req.method().clone(),
            path,
            ..Default::default()
        };

        if let Some(path) = req.uri().query() {
            let params: BTreeMap<String, String> = qs::from_str(path)?;
            creq.params = qs::to_string(params)?;
        }

        let mut canonical_headers = req.headers().clone();
        canonical_headers.remove("x-amz-date");
        canonical_headers.remove("x-amz-security-token");
        let x_amz_date = HeaderName::from_static("x-amz-date");
        let date_str = date.fmt_aws();
        let date_header = HeaderValue::from_str(&date_str).expect("date is valid header value");
        canonical_headers.insert(
            x_amz_date,
            date_header
        );
        let mut out = AddedHeaders {
            x_amz_date: date_str,
            x_amz_content_256: None,
            x_amz_security_token: None
        };

        if let Some(security_token) = security_token {
            canonical_headers.insert(
                "x-amz-security-token",
                HeaderValue::from_str(security_token)?,
            );
            out.x_amz_security_token = Some(security_token.to_string());
        }
        
        let payload_hash = match body {
            SignableBody::Bytes(data) => encode_bytes_with_hex(data),
            SignableBody::Precomputed(digest) => digest.clone(),
            SignableBody::UnsignedPayload => "UNSIGNED-PAYLOAD".to_string(),
        };
        creq.payload_hash = payload_hash.clone();
        if settings.signed_body_header == SignedBodyHeaderType::XAmzSha256 {
            let as_header_value = HeaderValue::from_str(&creq.payload_hash)?;
            canonical_headers.insert("x-amz-content-sha256", as_header_value.clone());
            out.x_amz_content_256 = Some(creq.payload_hash.clone());
        }

        let mut signed_headers = BTreeSet::new();
        for (name, _) in canonical_headers.iter() {
            signed_headers.insert(CanonicalHeaderName(name.clone()));
        }
        creq.signed_headers = SignedHeaders {
            inner: signed_headers,
        };
        creq.headers = canonical_headers;
        Ok((creq, out))
    }
}

impl AsSigV4 for CanonicalRequest {
    fn fmt(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for CanonicalRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.method)?;
        writeln!(f, "{}", self.path)?;
        writeln!(f, "{}", self.params)?;
        // write out _all_ the headers
        for header in &self.signed_headers.inner {
            // a missing header is a bug, so we should panic.
            let value = &self.headers[&header.0];
            write!(f, "{}:", header.0.as_str())?;
            writeln!(f, "{}", value.to_str().unwrap())?;
        }
        writeln!(f)?;
        // write out the signed headers
        write!(f, "{}", self.signed_headers.to_string())?;
        writeln!(f)?;
        write!(f, "{}", self.payload_hash)?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Default)]
pub(crate) struct SignedHeaders {
    inner: BTreeSet<CanonicalHeaderName>,
}

impl AsSigV4 for SignedHeaders {
    fn fmt(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for SignedHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut iter = self.inner.iter().peekable();
        while let Some(next) = iter.next() {
            match iter.peek().is_some() {
                true => write!(f, "{};", next.0.as_str())?,
                false => write!(f, "{}", next.0.as_str())?,
            };
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct CanonicalHeaderName(HeaderName);

impl PartialOrd for CanonicalHeaderName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CanonicalHeaderName {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.as_str().cmp(&other.0.as_str())
    }
}

#[derive(PartialEq, Debug, Clone)]
pub(crate) struct Scope<'a> {
    pub(crate) date: Date<Utc>,
    pub(crate) region: &'a str,
    pub(crate) service: &'a str,
}

impl<'a> AsSigV4 for Scope<'a> {
    fn fmt(&self) -> String {
        format!(
            "{}/{}/{}/aws4_request",
            self.date.fmt_aws(),
            self.region,
            self.service
        )
    }
}

impl<'a> TryFrom<&'a str> for Scope<'a> {
    type Error = Error;
    fn try_from(s: &'a str) -> Result<Scope<'a>, Self::Error> {
        let mut scopes = s.split('/');
        let date = Date::<Utc>::parse_aws(scopes.next().expect("missing date"))?;
        let region = scopes.next().expect("missing region");
        let service = scopes.next().expect("missing service");

        let scope = Scope {
            date,
            region,
            service,
        };

        Ok(scope)
    }
}

#[derive(PartialEq, Debug)]
pub(crate) struct StringToSign<'a> {
    pub(crate) scope: Scope<'a>,
    pub(crate) date: DateTime<Utc>,
    pub(crate) region: &'a str,
    pub(crate) service: &'a str,
    pub(crate) hashed_creq: &'a str,
}

impl<'a> TryFrom<&'a str> for StringToSign<'a> {
    type Error = Error;
    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        let lines = s.lines().collect::<Vec<&str>>();
        let date = DateTime::<Utc>::parse_aws(&lines[1])?;
        let scope: Scope = TryFrom::try_from(lines[2])?;
        let hashed_creq = &lines[3];

        let sts = StringToSign {
            date,
            region: scope.region,
            service: scope.service,
            scope,
            hashed_creq,
        };

        Ok(sts)
    }
}

impl<'a> StringToSign<'a> {
    pub(crate) fn new(
        date: DateTime<Utc>,
        region: &'a str,
        service: &'a str,
        hashed_creq: &'a str,
    ) -> Self {
        let scope = Scope {
            date: date.date(),
            region,
            service,
        };
        Self {
            scope,
            date,
            region,
            service,
            hashed_creq,
        }
    }
}

impl<'a> AsSigV4 for StringToSign<'a> {
    fn fmt(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}",
            HMAC_256,
            self.date.fmt_aws(),
            self.scope.fmt(),
            self.hashed_creq
        )
    }
}

pub(crate) trait DateTimeExt {
    // formats using SigV4's format. YYYYMMDD'T'HHMMSS'Z'.
    fn fmt_aws(&self) -> String;
    // YYYYMMDD
    fn parse_aws(s: &str) -> Result<DateTime<Utc>, ParseError>;
}

pub(crate) trait DateExt {
    fn fmt_aws(&self) -> String;

    fn parse_aws(s: &str) -> Result<Date<Utc>, ParseError>;
}

impl DateExt for Date<Utc> {
    fn fmt_aws(&self) -> String {
        self.format("%Y%m%d").to_string()
    }
    fn parse_aws(s: &str) -> Result<Date<Utc>, ParseError> {
        let date = NaiveDate::parse_from_str(s, "%Y%m%d")?;
        Ok(Date::<Utc>::from_utc(date, Utc))
    }
}

impl DateTimeExt for DateTime<Utc> {
    fn fmt_aws(&self) -> String {
        self.format(DATE_FORMAT).to_string()
    }

    fn parse_aws(s: &str) -> Result<DateTime<Utc>, ParseError> {
        let date = NaiveDateTime::parse_from_str(s, DATE_FORMAT)?;
        Ok(DateTime::<Utc>::from_utc(date, Utc))
    }
}
