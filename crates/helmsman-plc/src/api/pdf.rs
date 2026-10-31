//! PDF drop-ins (card `dropin-pdf`): an SOP, a manual page, a drawing.
//! The box never reads a PDF. It checks the two things it can know
//! without parsing the most hostile file format there is (the magic
//! number and the size), keeps the bytes, and hands them to the model
//! untouched as a base64 `document` block through the relay, which reads
//! the PDF on the model API's side (relay protocol v1, "PDF documents").
//!
//! **The relay's caps, held here too**, so a person hears "too big" at
//! upload and not as a failed chat turn:
//! - at most [`MAX_PDF`] bytes per PDF (decoded);
//! - at most [`MAX_PDFS`] PDFs per relay request, history included, so
//!   per conversation (the chat refuses the one past it,
//!   [`super::chat::Chat::attach`]).
//!
//! What the relay checks and the box does not: encryption and the page
//! count (100 per request). Both need the file's objects read; a PDF the
//! relay refuses for them fails that chat turn with the relay's message.

/// Largest PDF, bytes: relay protocol v1's per-PDF cap (2 MiB decoded).
pub const MAX_PDF: usize = 2 * 1024 * 1024;
/// Most PDFs one relay request carries, history included: relay
/// protocol v1's cap.
pub const MAX_PDFS: usize = 5;
/// What every PDF the relay takes starts with.
pub const MAGIC: &[u8] = b"%PDF-";
/// The `media_type` of the document block.
pub const MEDIA_TYPE: &str = "application/pdf";

/// Why a PDF is refused: an HTTP status and a reason for the person.
pub type Refused = (u16, String);

/// Whether `b` starts like a PDF, so it is never read as text.
#[must_use]
pub fn is_pdf(b: &[u8]) -> bool {
    b.starts_with(MAGIC)
}

/// Checks a PDF body against what the box can know without parsing it.
///
/// # Errors
/// 422 for an empty body, 413 over [`MAX_PDF`], 415 for bytes that do not
/// start with [`MAGIC`] (whatever the name or `Content-Type` says).
pub fn check(b: &[u8]) -> Result<(), Refused> {
    if b.is_empty() {
        return Err((422, "the PDF is empty".into()));
    }
    if b.len() > MAX_PDF {
        return Err((
            413,
            format!(
                "a PDF is at most {} MiB ({MAX_PDF} bytes), the relay's limit; send the pages that matter, or split it",
                MAX_PDF >> 20
            ),
        ));
    }
    if !is_pdf(b) {
        return Err((
            415,
            "this is not a PDF: its bytes do not start with %PDF- (the file name and Content-Type are not trusted)"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn a_pdf_is_known_by_its_magic_and_held_to_the_relays_size() {
        assert_eq!(check(b"%PDF-1.7\n%%EOF\n"), Ok(()));
        let mut at_cap = b"%PDF-1.4\n".to_vec();
        at_cap.resize(MAX_PDF, b' ');
        assert_eq!(check(&at_cap), Ok(()));
        at_cap.push(b' ');
        assert_eq!(check(&at_cap).unwrap_err().0, 413);
        assert_eq!(check(b"").unwrap_err().0, 422);
        for not in [
            &b"PK\x03\x04 a zip named .pdf"[..],
            b"<html>%PDF-1.4</html>",
            b" %PDF-1.4",
            b"%PDF",
            b"%pdf-1.4",
            b"\x89PNG\r\n\x1a\n",
        ] {
            assert_eq!(check(not).unwrap_err().0, 415, "{not:?}");
        }
    }

    proptest! {
        /// Whatever bytes arrive, the check answers without panicking, and
        /// only bytes that start with the magic within the cap pass.
        #[test]
        fn any_body_is_checked(body in prop::collection::vec(any::<u8>(), 0..64)) {
            let ok = check(&body).is_ok();
            prop_assert_eq!(ok, body.starts_with(MAGIC));
        }
    }
}
