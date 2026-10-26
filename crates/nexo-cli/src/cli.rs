//! Command tree of `nexo`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "nexo",
    version,
    about = "Zarządzanie Nexo CMS z terminala",
    long_about = "Zarządzanie Nexo CMS z terminala.\n\nZacznij od `nexo login --url https://cms.twoja-domena.pl`: nexo pokaże kod, który zatwierdzasz w panelu."
)]
pub struct Cli {
    /// Profil (instalacja CMS) z config.toml; domyślnie ustawiony albo jedyny.
    #[arg(long, short = 'p', global = true, env = "NEXO_PROFILE")]
    pub profile: Option<String>,
    /// Wynik jako JSON (do skryptów i jq).
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Logowanie tego komputera: kod zatwierdzasz w panelu.
    Login(LoginArgs),
    /// Wylogowuje ten komputer (kończy sesję także w panelu).
    Logout,
    /// Kto jest zalogowany, w którym profilu i dokąd trafiają zapytania.
    Status,
    /// Profile, czyli instalacje CMS, z którymi pracujesz.
    #[command(subcommand)]
    Profile(ProfileCommand),
    /// Twoje sesje (przeglądarki i terminale) i ich wylogowanie.
    #[command(subcommand)]
    Sessions(SessionsCommand),
    /// Witryny.
    #[command(subcommand)]
    Sites(SitesCommand),
    /// Strony, wpisy i inne dokumenty.
    #[command(subcommand, alias = "doc")]
    Docs(DocsCommand),
    /// Biblioteka mediów.
    #[command(subcommand)]
    Media(MediaCommand),
    /// Menu nawigacji.
    #[command(subcommand)]
    Menus(MenusCommand),
    /// Kategorie, tagi i inne taksonomie.
    #[command(subcommand, alias = "taxonomy")]
    Terms(TermsCommand),
    /// Formularze i ich zgłoszenia.
    #[command(subcommand)]
    Forms(FormsCommand),
    /// Kolekcje (np. handlowcy, oddziały) i ich wpisy.
    #[command(subcommand)]
    Collections(CollectionsCommand),
    /// Przekierowania adresów (301/302).
    #[command(subcommand)]
    Redirects(RedirectsCommand),
    /// Szablony stron rodzajów dokumentów (np. karta produktu).
    #[command(subcommand)]
    Layouts(LayoutsCommand),
    /// Wygląd witryny (motyw).
    #[command(subcommand)]
    Theme(ThemeCommand),
    /// Rozszerzenia i ich włączanie w witrynie.
    #[command(subcommand, alias = "ext")]
    Extensions(ExtensionsCommand),
    /// Sklep: produkty, zamówienia, klienci (moduł Sklep).
    #[command(subcommand)]
    Shop(ShopCommand),
    /// Użytkownicy i zaproszenia.
    #[command(subcommand)]
    Users(UsersCommand),
    /// Grupy uprawnień.
    #[command(subcommand)]
    Groups(GroupsCommand),
    /// Dziennik zdarzeń (kto, co i kiedy zmienił).
    Audit(AuditArgs),
    /// Zdjęcia produktów generowane przez AI (Gemini) z prawdziwego zdjęcia.
    #[command(subcommand)]
    Images(ImagesCommand),
    /// [zaawansowane] Dowolne wywołanie API: `nexo api GET /sites`.
    Api(ApiArgs),
    /// [zaawansowane] Wszystkie operacje API tej instalacji.
    Ops {
        /// Fragment nazwy, ścieżki albo opisu, np. menu, taxonomy, form.
        filter: Option<String>,
    },
    /// [zaawansowane] Operacja API po nazwie: `nexo call listMenus site=…`.
    Call(CallArgs),
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Adres panelu, np. https://cms.twoja-domena.pl (zapamiętany w profilu;
    /// bez niego nexo zapyta).
    #[arg(long, env = "NEXO_URL")]
    pub url: Option<String>,
    /// Domyślna witryna profilu (klucz albo site_…).
    #[arg(long)]
    pub site: Option<String>,
    /// Nazwa komputera widoczna w panelu (domyślnie nazwa hosta).
    #[arg(long)]
    pub device_name: Option<String>,
    /// Nie otwieraj przeglądarki, tylko pokaż link.
    #[arg(long)]
    pub no_browser: bool,
}

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// Lista profili.
    List,
    /// Ustawia profil domyślny.
    Use { name: String },
    /// Ustawia domyślną witrynę profilu (klucz albo site_…).
    Site { site: String },
    /// Usuwa profil i jego tokeny z tego komputera.
    Remove { name: String },
}

#[derive(Debug, Subcommand)]
pub enum SessionsCommand {
    /// Otwarte sesje.
    List,
    /// Wylogowuje sesję (ses_…).
    End { id: String },
}

#[derive(Debug, Args)]
pub struct ApiArgs {
    /// GET, POST, PUT, PATCH, DELETE.
    pub method: String,
    /// Ścieżka po /api/v1, np. /sites albo /documents/doc_…
    pub path: String,
    /// Parametr zapytania key=value (można powtarzać).
    #[arg(short = 'q', long = "query")]
    pub query: Vec<String>,
    /// Treść JSON albo @plik.json.
    #[arg(short = 'd', long = "data")]
    pub data: Option<String>,
}

#[derive(Debug, Args)]
pub struct CallArgs {
    /// operationId z `nexo ops`, np. listMenus, publishMenu, listTerms.
    pub operation: String,
    /// Parametry ścieżki i zapytania jako klucz=wartość.
    pub params: Vec<String>,
    /// Treść JSON albo @plik.json.
    #[arg(short = 'd', long = "data")]
    pub data: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum SitesCommand {
    /// Lista witryn.
    List,
    /// Szczegóły witryny.
    Show { site: Option<String> },
}

#[derive(Debug, Args)]
pub struct SiteArg {
    /// Witryna (klucz albo site_…); domyślnie z profilu.
    #[arg(long, short = 's')]
    pub site: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum DocsCommand {
    /// Lista dokumentów witryny.
    List {
        #[command(flatten)]
        site: SiteArg,
        /// Rodzaj: page, post, product…
        #[arg(long, short = 'k')]
        kind: Option<String>,
        /// draft, published, scheduled, archived.
        #[arg(long)]
        status: Option<String>,
        /// Szukaj w tytule.
        #[arg(long)]
        q: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Dokument ze szkicem (doc_… albo adres, np. /kontakt).
    Show {
        #[command(flatten)]
        site: SiteArg,
        doc: String,
    },
    /// Drzewo stron witryny.
    Tree {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Rodzaje dokumentów (strona, wpis, produkt…).
    Kinds,
    /// Tworzy dokument (szkic).
    Create {
        #[command(flatten)]
        site: SiteArg,
        /// Tytuł.
        title: String,
        #[arg(long, short = 'k', default_value = "page")]
        kind: String,
        /// Adres (slug); domyślnie z tytułu.
        #[arg(long)]
        slug: Option<String>,
        /// Strona nadrzędna (doc_… albo adres).
        #[arg(long)]
        parent: Option<String>,
        /// Język; domyślnie główny język witryny.
        #[arg(long)]
        locale: Option<String>,
    },
    /// Wycofuje publikację (strona znika z witryny).
    Unpublish {
        #[command(flatten)]
        site: SiteArg,
        doc: String,
        /// Archiwizuje zamiast cofać do szkicu.
        #[arg(long)]
        archive: bool,
    },
    /// Kopia dokumentu jako nowy szkic.
    Duplicate {
        #[command(flatten)]
        site: SiteArg,
        doc: String,
    },
    /// Przenosi dokument do kosza.
    Delete {
        #[command(flatten)]
        site: SiteArg,
        doc: String,
        /// Bez pytania o potwierdzenie.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Historia wersji dokumentu.
    Versions {
        #[command(flatten)]
        site: SiteArg,
        doc: String,
    },
    /// Przywraca wersję do szkicu.
    Restore {
        #[command(flatten)]
        site: SiteArg,
        doc: String,
        /// Numer wersji z `docs versions`.
        version: u32,
    },
    /// Publikuje bieżący szkic.
    Publish {
        #[command(flatten)]
        site: SiteArg,
        doc: String,
        /// Notatka do wersji.
        #[arg(long)]
        note: Option<String>,
    },
    /// Zapisuje szkic do pliku JSON (do edycji i `docs import`).
    Export {
        #[command(flatten)]
        site: SiteArg,
        doc: String,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
    /// Zapisuje szkic z pliku JSON (z `docs export`).
    Import {
        #[command(flatten)]
        site: SiteArg,
        doc: String,
        file: PathBuf,
        /// Od razu publikuje.
        #[arg(long)]
        publish: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum MediaCommand {
    /// Pliki w bibliotece.
    List {
        #[command(flatten)]
        site: SiteArg,
        /// Szukaj w nazwie, opisie alternatywnym i tytule.
        #[arg(long)]
        q: Option<String>,
        /// image, video, document.
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Wgrywa pliki albo całe foldery.
    Upload {
        #[command(flatten)]
        site: SiteArg,
        /// Pliki lub foldery.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Opis alternatywny dla wszystkich wgrywanych plików.
        #[arg(long)]
        alt: Option<String>,
    },
    /// Szczegóły pliku.
    Show { id: String },
    /// Zmienia opis alternatywny, tytuł, podpis albo nazwę pliku.
    Edit {
        id: String,
        #[arg(long)]
        alt: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        caption: Option<String>,
        #[arg(long)]
        filename: Option<String>,
    },
    /// Gdzie plik jest użyty.
    Usages { id: String },
    /// Usuwa plik (do kosza).
    Delete {
        id: String,
        /// Usuwa także, gdy plik jest użyty.
        #[arg(long)]
        force: bool,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Pobiera oryginał pliku (asset_…).
    Download {
        id: String,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ShopCommand {
    /// Produkty (dokumenty rodzaju product).
    Products {
        #[command(flatten)]
        site: SiteArg,
        #[arg(long)]
        q: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Produkt: dokument i dane sklepu (cena, stan, warianty).
    Product {
        #[command(flatten)]
        site: SiteArg,
        /// doc_… albo kod/nazwa produktu.
        product: String,
    },
    /// Zamówienia.
    Orders {
        #[command(flatten)]
        site: SiteArg,
        /// Status, np. new, paid, shipped, inquiry.
        #[arg(long)]
        status: Option<String>,
        /// Numer albo e-mail (fragment).
        #[arg(long)]
        q: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Zamówienie ze szczegółami (numer albo id).
    Order {
        #[command(flatten)]
        site: SiteArg,
        id: String,
    },
    /// Zmienia status zamówienia (np. paid, shipped, completed, cancelled).
    Status {
        #[command(flatten)]
        site: SiteArg,
        id: String,
        status: String,
    },
    /// Klienci sklepu.
    Customers {
        #[command(flatten)]
        site: SiteArg,
        #[arg(long)]
        q: Option<String>,
    },
    /// Eksport produktów do CSV (wiersz na wariant: SKU, cena, promocja, stan…).
    Export {
        #[command(flatten)]
        site: SiteArg,
        /// Plik .csv albo .json; bez tego CSV na ekran.
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
    /// Import cen, promocji, stanów i nazw z CSV (z `shop export`, wiersze po SKU).
    Import {
        #[command(flatten)]
        site: SiteArg,
        file: PathBuf,
        /// Pokazuje zmiany bez zapisywania.
        #[arg(long)]
        dry_run: bool,
        /// Publikuje produkty ze zmienioną nazwą.
        #[arg(long)]
        publish: bool,
    },
    /// Zdjęcia produktów: dodawanie i import z folderu.
    #[command(subcommand)]
    Photos(PhotosCommand),
}

#[derive(Debug, Subcommand)]
pub enum MenusCommand {
    /// Menu witryny.
    List {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Pozycje menu (klucz albo menu_…).
    Show {
        #[command(flatten)]
        site: SiteArg,
        menu: String,
        /// Język menu; domyślnie główny język witryny.
        #[arg(long)]
        locale: Option<String>,
    },
    /// Zapisuje pozycje menu do pliku JSON (do edycji i `menus import`).
    Export {
        #[command(flatten)]
        site: SiteArg,
        menu: String,
        /// Język menu; domyślnie główny język witryny.
        #[arg(long)]
        locale: Option<String>,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
    /// Wczytuje pozycje menu z pliku JSON (szkic).
    Import {
        #[command(flatten)]
        site: SiteArg,
        menu: String,
        /// Język menu; domyślnie główny język witryny.
        #[arg(long)]
        locale: Option<String>,
        file: PathBuf,
        /// Od razu publikuje.
        #[arg(long)]
        publish: bool,
    },
    /// Publikuje szkic menu.
    Publish {
        #[command(flatten)]
        site: SiteArg,
        menu: String,
        /// Język menu; domyślnie główny język witryny.
        #[arg(long)]
        locale: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum TermsCommand {
    /// Pojęcia taksonomii (np. kategorie).
    List {
        #[command(flatten)]
        site: SiteArg,
        /// Taksonomia, np. category, tag, styl.
        #[arg(long, short = 't')]
        taxonomy: Option<String>,
    },
    /// Dodaje pojęcie.
    Add {
        #[command(flatten)]
        site: SiteArg,
        name: String,
        #[arg(long, short = 't')]
        taxonomy: String,
        #[arg(long)]
        slug: Option<String>,
        /// Pojęcie nadrzędne (nazwa, slug albo term_…).
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Import z CSV: kolumny name[,slug,parent,description]; istniejące pomija.
    Import {
        #[command(flatten)]
        site: SiteArg,
        file: PathBuf,
        #[arg(long, short = 't')]
        taxonomy: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Zmienia nazwę, adres albo opis pojęcia.
    Edit {
        #[command(flatten)]
        site: SiteArg,
        term: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Usuwa pojęcie.
    Delete {
        #[command(flatten)]
        site: SiteArg,
        term: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum FormsCommand {
    /// Formularze witryny z liczbą zgłoszeń.
    List {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Pola i ustawienia formularza (klucz albo form_…).
    Show {
        #[command(flatten)]
        site: SiteArg,
        form: String,
    },
    /// Zgłoszenia (wszystkie albo jednego formularza).
    Submissions {
        #[command(flatten)]
        site: SiteArg,
        /// Formularz (klucz albo form_…).
        form: Option<String>,
        /// new, read, archived, spam.
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        q: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Treść zgłoszenia (sub_…); oznacza je jako przeczytane.
    Submission { id: String },
    /// Zmienia status zgłoszenia.
    Mark {
        id: String,
        /// read, new, archived, spam.
        status: String,
    },
    /// Zgłoszenia formularza do pliku CSV.
    Export {
        #[command(flatten)]
        site: SiteArg,
        form: String,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum CollectionsCommand {
    /// Kolekcje witryny.
    List {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Wpisy kolekcji (klucz albo coll_…).
    Entries {
        #[command(flatten)]
        site: SiteArg,
        collection: String,
        #[arg(long)]
        q: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Dodaje wpis z JSON (pola kolekcji).
    Add {
        #[command(flatten)]
        site: SiteArg,
        collection: String,
        /// Pola jako JSON albo @plik.json.
        #[arg(short = 'd', long = "data")]
        data: String,
        /// Od razu widoczny na stronie.
        #[arg(long)]
        publish: bool,
    },
    /// Import wpisów z CSV: nagłówki kolumn to nazwy pól kolekcji.
    Import {
        #[command(flatten)]
        site: SiteArg,
        collection: String,
        file: PathBuf,
        /// Wpisy od razu widoczne na stronie.
        #[arg(long)]
        publish: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Usuwa wpis (entry_…).
    Delete {
        id: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Pobiera wpisy kolekcji z jej źródła (synchronizacja).
    Sync {
        #[command(flatten)]
        site: SiteArg,
        collection: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RedirectsCommand {
    /// Przekierowania witryny.
    List {
        #[command(flatten)]
        site: SiteArg,
        #[arg(long)]
        q: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Dodaje przekierowanie: nexo redirects add /stary-adres /nowy-adres
    Add {
        #[command(flatten)]
        site: SiteArg,
        from: String,
        /// Adres docelowy (pusty z --code 410: strona usunięta).
        to: Option<String>,
        /// 301 (stałe), 302 (tymczasowe) albo 410 (usunięta).
        #[arg(long, default_value_t = 301)]
        code: u16,
    },
    /// Import z CSV: kolumny from,to[,code] (np. stare adresy WooCommerce).
    Import {
        #[command(flatten)]
        site: SiteArg,
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Eksport do CSV.
    Export {
        #[command(flatten)]
        site: SiteArg,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
    /// Usuwa przekierowanie (rdr_… albo adres źródłowy).
    Delete {
        #[command(flatten)]
        site: SiteArg,
        redirect: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum LayoutsCommand {
    /// Szablony witryny.
    List {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Bloki szablonu (lay_… albo rodzaj, np. product).
    Show {
        #[command(flatten)]
        site: SiteArg,
        layout: String,
    },
    /// Publikuje szkic szablonu.
    Publish {
        #[command(flatten)]
        site: SiteArg,
        layout: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ThemeCommand {
    /// Ustawienia wyglądu (szkic i opublikowane).
    Show {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Publikuje szkic wyglądu.
    Publish {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Dostępne motywy.
    List,
}

#[derive(Debug, Subcommand)]
pub enum ExtensionsCommand {
    /// Rozszerzenia i to, czy są włączone w witrynie.
    List {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Włącza rozszerzenie w witrynie (z wymaganymi).
    Enable {
        #[command(flatten)]
        site: SiteArg,
        extension: String,
    },
    /// Wyłącza rozszerzenie w witrynie (dane zostają).
    Disable {
        #[command(flatten)]
        site: SiteArg,
        extension: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum PhotosCommand {
    /// Dodaje zdjęcia do galerii produktu.
    Add {
        #[command(flatten)]
        site: SiteArg,
        /// doc_… albo kod/nazwa produktu.
        product: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Na początek galerii (pierwsze zdjęcie to zdjęcie główne).
        #[arg(long)]
        first: bool,
        /// Zastępuje całą galerię.
        #[arg(long, conflicts_with = "first")]
        replace: bool,
        #[arg(long)]
        publish: bool,
    },
    /// Wgrywa folder zdjęć i przypisuje je produktom po kodzie z nazwy pliku
    /// (MON001.jpg, MON001_2.jpg, MON001-tyl.webp → produkt z kodem MON001).
    Import {
        #[command(flatten)]
        site: SiteArg,
        folder: PathBuf,
        /// Zastępuje galerie zamiast dopisywać zdjęcia na końcu.
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        publish: bool,
        /// Pokazuje przypisania bez wgrywania.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum UsersCommand {
    /// Użytkownicy.
    List {
        #[arg(long)]
        q: Option<String>,
        /// active albo disabled.
        #[arg(long)]
        status: Option<String>,
    },
    /// Konto (e-mail albo usr_…).
    Show { user: String },
    /// Zaprasza osobę: tworzy konto i wysyła link do ustawienia hasła.
    Invite {
        email: String,
        /// Imię i nazwisko (domyślnie część adresu przed @).
        #[arg(long)]
        name: Option<String>,
        /// Grupa (nazwa albo grp_…), można powtarzać.
        #[arg(long = "group")]
        groups: Vec<String>,
    },
    /// Wyłącza konto (kończy też wszystkie jego sesje).
    Disable {
        user: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Włącza wyłączone konto.
    Enable { user: String },
    /// Wysyła ponownie link do ustawienia hasła.
    ResendInvite { user: String },
    /// Usuwa aplikację uwierzytelniającą (zgubiony telefon).
    ResetTotp {
        user: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum GroupsCommand {
    /// Grupy z liczbą osób.
    List,
    /// Uprawnienia i witryny grupy (nazwa albo grp_…).
    Show { group: String },
}

#[derive(Debug, Args)]
pub struct AuditArgs {
    #[command(flatten)]
    pub site: SiteArg,
    /// Tylko zdarzenia tej osoby (e-mail albo usr_…).
    #[arg(long)]
    pub user: Option<String>,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

#[derive(Debug, Subcommand)]
pub enum ImagesCommand {
    /// Zapisuje klucz Gemini API w pęku kluczy systemu.
    Setup,
    /// Prompty presetów: wbudowane, w CMS witryny albo na tym komputerze.
    #[command(subcommand)]
    Prompts(PromptsCommand),
    /// Generuje zdjęcia produktu z jego prawdziwych zdjęć.
    Generate(GenerateArgs),
}

#[derive(Debug, Subcommand)]
pub enum PromptsCommand {
    /// Skąd każdy preset bierze prompt.
    List {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Zapisuje prompty w CMS witryny (niepubliczna kolekcja, edytowalna w
    /// panelu): każdy zalogowany komputer używa ich bez kopiowania plików.
    Push {
        #[command(flatten)]
        site: SiteArg,
        /// Folder z plikami lifestyle_room.md / warm_packshot.md (domyślnie
        /// lokalne prompty tego komputera).
        #[arg(long)]
        from: Option<PathBuf>,
    },
    /// Pobiera prompty z CMS na ten komputer (do edycji).
    Pull {
        #[command(flatten)]
        site: SiteArg,
    },
    /// Ustawia prompt presetu tylko na tym komputerze (ma pierwszeństwo przed CMS).
    Set {
        /// lifestyle-room albo warm-packshot.
        preset: String,
        /// Plik .md; bez niego usuwa lokalny prompt.
        file: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
pub struct GenerateArgs {
    #[command(flatten)]
    pub site: SiteArg,
    /// Produkty: doc_… albo kod/nazwa (można podać kilka).
    #[arg(required = true)]
    pub products: Vec<String>,
    /// lifestyle-room, warm-packshot albo oba (domyślnie oba, jak w cr).
    #[arg(long, value_delimiter = ',')]
    pub preset: Vec<String>,
    /// Zdjęcie źródłowe: fragment nazwy pliku (np. LET73G_02). Pierwsze dla
    /// aranżacji, drugie dla packshotu; domyślnie pierwsze własne zdjęcia galerii.
    #[arg(long = "source", value_delimiter = ',')]
    pub sources: Vec<String>,
    /// Folder na wyniki.
    #[arg(long, default_value = "nexo-images")]
    pub out: PathBuf,
    /// Wgrywa wyniki do biblioteki mediów.
    #[arg(long)]
    pub upload: bool,
    /// Wstawia wyniki do galerii produktu (zapisuje szkic; wymaga --upload).
    #[arg(long, requires = "upload")]
    pub replace: bool,
    /// Publikuje produkt po podmianie galerii.
    #[arg(long, requires = "replace")]
    pub publish: bool,
    /// Pokazuje, co by zrobił, bez wywołań Gemini.
    #[arg(long)]
    pub dry_run: bool,
}
