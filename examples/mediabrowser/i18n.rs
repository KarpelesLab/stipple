//! The app's strings in English, French, Japanese and Arabic (through
//! `stipple::locale::Catalog`), and the formats that follow the locale:
//! sizes, dates, durations and counts.

use std::time::Duration;
use stipple::locale::{Catalog, DateStyle, DateTime, Locale, TimeStyle, negotiate};
use stipple::platform;

/// The languages the app ships, its own first: (tag, name in itself).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("fr", "Français"),
    ("ja", "日本語"),
    ("ar", "العربية"),
];

pub fn shipped() -> Vec<&'static str> {
    LANGUAGES.iter().map(|(tag, _)| *tag).collect()
}

const EN: &str = "
app.title = Media Browser
menu.file = &File
menu.open = &Open Folder…
menu.sample = Open &Sample Folder
menu.refresh = &Refresh
menu.quit = &Quit
menu.edit = &Edit
menu.copy_path = &Copy Path
menu.select_all = Select &All
menu.view = &View
menu.list = As &List
menu.grid = As &Grid
menu.theme = &Theme
menu.language = &Language
theme.system = System
theme.light = Light
theme.dark = Dark
lang.system = System language
menu.go = &Go
menu.up = &Enclosing Folder
menu.playback = &Playback
play.toggle = &Play/Pause
play.stop = &Stop
ctx.open = Open
ctx.copy_path = Copy Path
ctx.reveal = Show in Folder
ctx.play = Play
tool.open = Open Folder…
tool.up = Up
tool.refresh = Refresh
tool.location = Location
view.list = List
view.grid = Grid
col.name = Name
col.size = Size
col.kind = Kind
col.modified = Modified
kind.folder = Folder
kind.image = Image
kind.audio = Audio
kind.markdown = Markdown
kind.text = Text
kind.other = File
size.bytes.one = {count} byte
size.bytes.other = {count} bytes
size.kb = {n} KB
size.mb = {n} MB
size.gb = {n} GB
items.one = {count} item
items.other = {count} items
selected.one = {count} selected
selected.other = {count} selected
status = {items} · {selected}
status.copied = Path copied: {path}
status.dropped.one = {count} file copied here
status.dropped.other = {count} files copied here
status.loading.one = loading {count} preview
status.loading.other = loading {count} previews
pane.folders = Folders
pane.files = Files
pane.preview = Preview
preview.none = Select a file to see it here.
preview.loading = Loading…
preview.failed = This file can't be shown.
empty = This folder is empty.
meta.kind = Kind
meta.size = Size
meta.modified = Modified
meta.location = Location
meta.dimensions = Dimensions
meta.dim = {w} × {h} pixels
meta.duration = Duration
meta.channels = Channels
meta.rate = Sample rate
meta.hz = {n} Hz
meta.lines = Lines
meta.words = Words
meta.contains = Contains
player.play = Play
player.pause = Pause
player.stop = Stop
player.position = Position, {at} of {total}
";

const FR: &str = "
app.title = Navigateur de médias
menu.file = &Fichier
menu.open = &Ouvrir un dossier…
menu.sample = Ouvrir le dossier d’e&xemple
menu.refresh = &Actualiser
menu.quit = &Quitter
menu.edit = É&dition
menu.copy_path = &Copier le chemin
menu.select_all = &Tout sélectionner
menu.view = &Présentation
menu.list = En &liste
menu.grid = En &grille
menu.theme = &Thème
menu.language = &Langue
theme.system = Système
theme.light = Clair
theme.dark = Sombre
lang.system = Langue du système
menu.go = A&ller
menu.up = Dossier &parent
menu.playback = &Lecture
play.toggle = &Lecture/Pause
play.stop = &Arrêter
ctx.open = Ouvrir
ctx.copy_path = Copier le chemin
ctx.reveal = Afficher dans le dossier
ctx.play = Lire
tool.open = Ouvrir un dossier…
tool.up = Haut
tool.refresh = Actualiser
tool.location = Emplacement
view.list = Liste
view.grid = Grille
col.name = Nom
col.size = Taille
col.kind = Type
col.modified = Modifié
kind.folder = Dossier
kind.image = Image
kind.audio = Son
kind.markdown = Markdown
kind.text = Texte
kind.other = Fichier
size.bytes.one = {count} octet
size.bytes.other = {count} octets
size.kb = {n} Ko
size.mb = {n} Mo
size.gb = {n} Go
items.one = {count} élément
items.other = {count} éléments
selected.one = {count} sélectionné
selected.other = {count} sélectionnés
status = {items} · {selected}
status.copied = Chemin copié : {path}
status.dropped.one = {count} fichier copié ici
status.dropped.other = {count} fichiers copiés ici
status.loading.one = {count} aperçu en cours
status.loading.other = {count} aperçus en cours
pane.folders = Dossiers
pane.files = Fichiers
pane.preview = Aperçu
preview.none = Sélectionnez un fichier pour le voir ici.
preview.loading = Chargement…
preview.failed = Ce fichier ne peut pas être affiché.
empty = Ce dossier est vide.
meta.kind = Type
meta.size = Taille
meta.modified = Modifié
meta.location = Emplacement
meta.dimensions = Dimensions
meta.dim = {w} × {h} pixels
meta.duration = Durée
meta.channels = Canaux
meta.rate = Fréquence
meta.hz = {n} Hz
meta.lines = Lignes
meta.words = Mots
meta.contains = Contenu
player.play = Lire
player.pause = Pause
player.stop = Arrêter
player.position = Position, {at} sur {total}
";

const JA: &str = "
app.title = メディアブラウザ
menu.file = ファイル(&F)
menu.open = フォルダを開く(&O)…
menu.sample = サンプルフォルダを開く(&S)
menu.refresh = 更新(&R)
menu.quit = 終了(&Q)
menu.edit = 編集(&E)
menu.copy_path = パスをコピー(&C)
menu.select_all = すべて選択(&A)
menu.view = 表示(&V)
menu.list = リスト(&L)
menu.grid = グリッド(&G)
menu.theme = テーマ(&T)
menu.language = 言語(&L)
theme.system = システム
theme.light = ライト
theme.dark = ダーク
lang.system = システムの言語
menu.go = 移動(&G)
menu.up = 親フォルダ(&U)
menu.playback = 再生(&P)
play.toggle = 再生/一時停止(&P)
play.stop = 停止(&S)
ctx.open = 開く
ctx.copy_path = パスをコピー
ctx.reveal = フォルダに表示
ctx.play = 再生
tool.open = フォルダを開く…
tool.up = 上へ
tool.refresh = 更新
tool.location = 場所
view.list = リスト
view.grid = グリッド
col.name = 名前
col.size = サイズ
col.kind = 種類
col.modified = 更新日時
kind.folder = フォルダ
kind.image = 画像
kind.audio = オーディオ
kind.markdown = Markdown
kind.text = テキスト
kind.other = ファイル
size.bytes.other = {count} バイト
size.kb = {n} KB
size.mb = {n} MB
size.gb = {n} GB
items.other = {count} 項目
selected.other = {count} 個を選択
status = {items}・{selected}
status.copied = パスをコピーしました: {path}
status.dropped.other = {count} 個のファイルをコピーしました
status.loading.other = {count} 件のプレビューを読み込み中
pane.folders = フォルダ
pane.files = ファイル
pane.preview = プレビュー
preview.none = ファイルを選択するとここに表示されます。
preview.loading = 読み込み中…
preview.failed = このファイルは表示できません。
empty = このフォルダは空です。
meta.kind = 種類
meta.size = サイズ
meta.modified = 更新日時
meta.location = 場所
meta.dimensions = 寸法
meta.dim = {w} × {h} ピクセル
meta.duration = 長さ
meta.channels = チャンネル
meta.rate = サンプルレート
meta.hz = {n} Hz
meta.lines = 行数
meta.words = 単語数
meta.contains = 内容
player.play = 再生
player.pause = 一時停止
player.stop = 停止
player.position = 再生位置 {at} / {total}
";

const AR: &str = "
app.title = متصفح الوسائط
menu.file = &ملف
menu.open = &فتح مجلد…
menu.sample = فتح مجلد ال&أمثلة
menu.refresh = &تحديث
menu.quit = &إنهاء
menu.edit = &تحرير
menu.copy_path = &نسخ المسار
menu.select_all = تحديد ال&كل
menu.view = &عرض
menu.list = &قائمة
menu.grid = &شبكة
menu.theme = ال&سمة
menu.language = الل&غة
theme.system = النظام
theme.light = فاتحة
theme.dark = داكنة
lang.system = لغة النظام
menu.go = ا&نتقال
menu.up = المجلد الأ&صل
menu.playback = ت&شغيل
play.toggle = تشغيل/إي&قاف مؤقت
play.stop = إي&قاف
ctx.open = فتح
ctx.copy_path = نسخ المسار
ctx.reveal = إظهار في المجلد
ctx.play = تشغيل
tool.open = فتح مجلد…
tool.up = لأعلى
tool.refresh = تحديث
tool.location = الموقع
view.list = قائمة
view.grid = شبكة
col.name = الاسم
col.size = الحجم
col.kind = النوع
col.modified = آخر تعديل
kind.folder = مجلد
kind.image = صورة
kind.audio = صوت
kind.markdown = ماركداون
kind.text = نص
kind.other = ملف
size.bytes.zero = {count} بايت
size.bytes.one = بايت واحد
size.bytes.two = بايتان
size.bytes.few = {count} بايتات
size.bytes.many = {count} بايتًا
size.bytes.other = {count} بايت
size.kb = {n} ك.ب
size.mb = {n} م.ب
size.gb = {n} غ.ب
items.zero = لا عناصر
items.one = عنصر واحد
items.two = عنصران
items.few = {count} عناصر
items.many = {count} عنصرًا
items.other = {count} عنصر
selected.zero = لا شيء محدد
selected.one = عنصر واحد محدد
selected.two = عنصران محددان
selected.few = {count} عناصر محددة
selected.many = {count} عنصرًا محددًا
selected.other = {count} عنصر محدد
status = {items} · {selected}
status.copied = تم نسخ المسار: {path}
status.dropped.one = تم نسخ ملف واحد هنا
status.dropped.other = تم نسخ {count} ملفات هنا
status.loading.other = جارٍ تحميل {count} معاينات
pane.folders = المجلدات
pane.files = الملفات
pane.preview = معاينة
preview.none = حدد ملفًا لعرضه هنا.
preview.loading = جارٍ التحميل…
preview.failed = لا يمكن عرض هذا الملف.
empty = هذا المجلد فارغ.
meta.kind = النوع
meta.size = الحجم
meta.modified = آخر تعديل
meta.location = الموقع
meta.dimensions = الأبعاد
meta.dim = {w} × {h} بكسل
meta.duration = المدة
meta.channels = القنوات
meta.rate = معدل العينة
meta.hz = {n} هرتز
meta.lines = الأسطر
meta.words = الكلمات
meta.contains = المحتوى
player.play = تشغيل
player.pause = إيقاف مؤقت
player.stop = إيقاف
player.position = الموضع {at} من {total}
";

/// The strings of one language and the locale that formats for it.
#[derive(Debug)]
pub struct Tr {
    pub catalog: Catalog,
    pub locale: Locale,
    /// The language shown (one of [`LANGUAGES`]).
    pub language: &'static str,
}

impl Tr {
    /// The strings for `choice` (a tag of [`LANGUAGES`]), or for the user's
    /// own languages when `None`.
    pub fn new(choice: Option<&str>) -> Tr {
        let shipped = shipped();
        let (language, locale) = match choice {
            Some(tag) => {
                let lang = negotiate(&[tag], &shipped).unwrap_or("en");
                (lang, Locale::new(default_region(lang)))
            }
            None => {
                let lang = negotiate(&platform::preferred_languages(), &shipped).unwrap_or("en");
                let user = platform::locale();
                // The user's conventions when they read that language, else
                // the language's own.
                let locale = if user.language() == lang {
                    user
                } else {
                    Locale::new(default_region(lang))
                };
                (lang, locale)
            }
        };
        let en = Catalog::parse("en", EN).expect("English catalog");
        let mut catalog = match language {
            "fr" => Catalog::parse("fr", FR)
                .expect("French catalog")
                .fallback(en),
            "ja" => Catalog::parse("ja", JA)
                .expect("Japanese catalog")
                .fallback(en),
            "ar" => Catalog::parse("ar", AR)
                .expect("Arabic catalog")
                .fallback(en),
            _ => en,
        };
        catalog.set_locale(locale.clone());
        Tr {
            catalog,
            locale,
            language,
        }
    }

    pub fn t(&self, key: &str) -> String {
        self.catalog.text(key)
    }

    pub fn f(&self, key: &str, args: &[(&str, &str)]) -> String {
        self.catalog.format(key, args)
    }

    pub fn plural(&self, key: &str, n: usize) -> String {
        self.catalog.plural(key, n as f64, &[])
    }

    /// A file size: bytes below 1 KiB, else KB, MB or GB with a decimal.
    pub fn size(&self, bytes: u64) -> String {
        const K: f64 = 1024.0;
        let b = bytes as f64;
        if b < K {
            return self.plural("size.bytes", bytes as usize);
        }
        let (n, key) = if b < K * K {
            (b / K, "size.kb")
        } else if b < K * K * K {
            (b / (K * K), "size.mb")
        } else {
            (b / (K * K * K), "size.gb")
        };
        self.f(key, &[("n", &self.locale.format_fixed(n, 1))])
    }

    /// A modification time, in the user's time zone.
    pub fn date(&self, unix: i64) -> String {
        let dt = DateTime::from_unix_local(unix);
        self.locale
            .format_date_time(&dt, DateStyle::Medium, TimeStyle::Short)
    }

    /// A duration as m:ss, in the locale's digits.
    pub fn duration(&self, d: Duration) -> String {
        let secs = d.as_secs();
        let (m, s) = (secs / 60, secs % 60);
        let n = |x: u64| self.locale.format_number(x as f64);
        let ss = if s < 10 {
            format!("{}{}", n(0), n(s))
        } else {
            n(s)
        };
        format!("{}:{ss}", n(m))
    }

    /// A plain number in the locale's digits and grouping.
    pub fn number(&self, n: u64) -> String {
        self.locale.format_number(n as f64)
    }
}

/// The region whose conventions a language gets when the app picks it.
fn default_region(lang: &str) -> &'static str {
    match lang {
        "fr" => "fr-FR",
        "ja" => "ja-JP",
        "ar" => "ar-EG",
        _ => "en-US",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_every_key() {
        let keys: Vec<&str> = EN
            .lines()
            .filter_map(|l| l.split_once(" = ").map(|(k, _)| k.trim()))
            .filter(|k| !k.contains(".one") && !k.contains(".other"))
            .collect();
        for (lang, text) in [("fr", FR), ("ja", JA), ("ar", AR)] {
            let c = Catalog::parse(lang, text).unwrap();
            for key in &keys {
                assert!(c.contains(key), "{lang} lacks {key}");
            }
        }
    }

    #[test]
    fn sizes_dates_and_counts_follow_the_locale() {
        let en = Tr::new(Some("en"));
        let fr = Tr::new(Some("fr"));
        let ar = Tr::new(Some("ar"));
        assert_eq!(en.size(1536), "1.5 KB");
        assert_eq!(fr.size(1536), "1,5 Ko");
        assert_eq!(ar.size(1536), "١٫٥ ك.ب");
        assert_eq!(en.plural("items", 1), "1 item");
        assert_eq!(fr.plural("items", 2), "2 éléments");
        assert_eq!(ar.plural("items", 2), "عنصران");
        assert_eq!(en.duration(Duration::from_secs(65)), "1:05");
        assert_eq!(ar.duration(Duration::from_secs(65)), "١:٠٥");
        // 2026-11-17 12:00 UTC; the date part doesn't depend on the zone.
        let date = fr.date(1_794_916_800);
        assert!(date.contains("nov. 2026"), "{date}");
    }
}
