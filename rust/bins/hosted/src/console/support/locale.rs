//! Purpose: localize enterprise responses with console locale negotiation rules.
//! Scope: pure request-header interpretation and baseline translation strings.
//! Uses:
//! `-- none
//! External:
//! `-- axum::http::HeaderMap
//! Used by:
//! rust/bins/
//! `-- hosted/src/console/support/enterprise.rs
//! Invariants: explicit locale header wins over cookie, which wins over weighted Accept-Language.
//! Contract tests: hosted/src/console/support/tests.rs

use axum::http::HeaderMap;

const LOCALES: &[&str] = &[
    "en", "zh", "zht", "ko", "de", "es", "fr", "it", "da", "ja", "pl", "ru", "uk", "ar", "no",
    "br", "th", "tr",
];

pub(super) fn messages(headers: &HeaderMap) -> [&'static str; 4] {
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let explicit = header("x-opencode-locale").filter(|v| LOCALES.contains(v));
    let cookie = header("cookie")
        .and_then(|v| {
            v.split(';')
                .map(str::trim)
                .find_map(|v| v.strip_prefix("oc_locale="))
        })
        .filter(|v| LOCALES.contains(v));
    let mut languages = header("accept-language")
        .unwrap_or_default()
        .split(',')
        .map(|raw| {
            let mut parts = raw.trim().split(';');
            let language = parts.next().unwrap_or_default();
            let quality = parts
                .find_map(|part| part.trim().strip_prefix("q="))
                .and_then(|q| q.parse::<f64>().ok())
                .unwrap_or(1.0);
            (language, quality)
        })
        .collect::<Vec<_>>();
    languages.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let detected = languages.iter().find_map(|(language, _)| {
        let value = language.to_lowercase();
        if value.starts_with("zh") {
            return Some(
                if ["hant", "-tw", "-hk", "-mo"]
                    .iter()
                    .any(|v| value.contains(v))
                {
                    "zht"
                } else {
                    "zh"
                },
            );
        }
        if value.starts_with("pt") {
            return Some("br");
        }
        if ["no", "nb", "nn"].iter().any(|v| value.starts_with(v)) {
            return Some("no");
        }
        LOCALES
            .iter()
            .copied()
            .find(|locale| value.starts_with(locale))
    });
    match explicit.or(cookie).or(detected).unwrap_or("en") {
        "zh" => [
            "表单提交成功。",
            "所有字段均为必填项。",
            "邮箱格式无效。",
            "内部服务器错误。",
        ],
        "zht" => [
            "表單已成功送出。",
            "所有欄位均為必填。",
            "無效的電子郵件格式。",
            "內部伺服器錯誤。",
        ],
        "ko" => [
            "양식이 성공적으로 제출되었습니다.",
            "모든 필드는 필수 항목입니다.",
            "유효하지 않은 이메일 형식입니다.",
            "내부 서버 오류입니다.",
        ],
        "de" => [
            "Formular erfolgreich gesendet.",
            "Alle Felder sind erforderlich.",
            "Ungültiges E-Mail-Format.",
            "Interner Serverfehler.",
        ],
        "es" => [
            "Formulario enviado con éxito.",
            "Todos los campos son obligatorios.",
            "Formato de correo inválido.",
            "Error interno del servidor.",
        ],
        "fr" => [
            "Formulaire soumis avec succès.",
            "Tous les champs sont requis.",
            "Format d'e-mail invalide.",
            "Erreur interne du serveur.",
        ],
        "it" => [
            "Modulo inviato con successo.",
            "Tutti i campi sono obbligatori.",
            "Formato email non valido.",
            "Errore interno del server.",
        ],
        "da" => [
            "Formular indsendt med succes.",
            "Alle felter er påkrævet.",
            "Ugyldigt e-mailformat.",
            "Intern serverfejl.",
        ],
        "ja" => [
            "フォームが正常に送信されました。",
            "すべての項目は必須です。",
            "無効なメール形式です。",
            "内部サーバーエラー。",
        ],
        "pl" => [
            "Formularz został pomyślnie wysłany.",
            "Wszystkie pola są wymagane.",
            "Nieprawidłowy format adresu e-mail.",
            "Wewnętrzny błąd serwera.",
        ],
        "ru" => [
            "Форма успешно отправлена.",
            "Все поля обязательны.",
            "Неверный формат email.",
            "Внутренняя ошибка сервера.",
        ],
        "uk" => [
            "Форму успішно надіслано.",
            "Усі поля обов'язкові.",
            "Недійсний формат email.",
            "Внутрішня помилка сервера.",
        ],
        "ar" => [
            "تم إرسال النموذج بنجاح.",
            "جميع الحقول مطلوبة.",
            "تنسيق البريد الإلكتروني غير صالح.",
            "خطأ داخلي في الخادم.",
        ],
        "no" => [
            "Skjemaet ble sendt inn.",
            "Alle felt er obligatoriske.",
            "Ugyldig e-postformat.",
            "Intern serverfeil.",
        ],
        "br" => [
            "Formulário enviado com sucesso.",
            "Todos os campos são obrigatórios.",
            "Formato de e-mail inválido.",
            "Erro interno do servidor.",
        ],
        "th" => [
            "ส่งแบบฟอร์มสำเร็จแล้ว",
            "จำเป็นต้องกรอกทุกช่อง",
            "รูปแบบอีเมลไม่ถูกต้อง",
            "เกิดข้อผิดพลาดภายในเซิร์ฟเวอร์",
        ],
        "tr" => [
            "Form başarıyla gönderildi.",
            "Tüm alanlar gereklidir.",
            "Geçersiz e-posta formatı.",
            "İç sunucu hatası.",
        ],
        _ => [
            "Form submitted successfully.",
            "All fields are required.",
            "Invalid email format.",
            "Internal server error.",
        ],
    }
}
