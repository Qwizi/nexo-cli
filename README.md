# nexo

Zarządzanie [Nexo CMS](https://github.com/Qwizi/nexo-cms) z terminala: strony,
media, menu, taksonomie, formularze, kolekcje, przekierowania, wygląd,
rozszerzenia, sklep, użytkownicy i dziennik zdarzeń, a do tego zdjęcia
produktów generowane przez AI.

## Instalacja

Jedna komenda, bez Rusta: pobiera gotowy plik dla Twojego systemu, sprawdza
sumę kontrolną, dodaje `nexo` do PATH i pyta o adres panelu do zalogowania.

macOS i Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/Qwizi/nexo-cli/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/Qwizi/nexo-cli/main/install.ps1 | iex
```

Opcje: `NEXO_URL=https://cms.twoja-domena.pl` (zaloguj bez pytania),
`NEXO_VERSION=v0.2.0`, `NEXO_INSTALL_DIR=…`, `NEXO_NO_LOGIN=1`. Ponowne
uruchomienie aktualizuje do najnowszej wersji.

Ze źródeł (Rust 1.89+): `cargo install --git https://github.com/Qwizi/nexo-cli nexo-cli`.

## Logowanie

```bash
nexo login --url https://cms.twoja-domena.pl
```

nexo pokazuje kod i otwiera stronę panelu `/urzadzenie`, gdzie sprawdzasz kod
i klikasz **Zezwól**. Terminal działa wtedy z Twoimi uprawnieniami. Sesję widać
w panelu w menu konta › **Sesje i urządzenia**, gdzie można ją wylogować;
`nexo logout` robi to z terminala.

Tokeny są w pęku kluczy systemu (Keychain, Windows Credential Manager, keyring
jądra Linuksa); bez niego w pliku czytelnym tylko dla właściciela
(`NEXO_TOKEN_STORE=file` wymusza plik, np. w CI).

Kilka instalacji to kilka profili:

```bash
nexo login --url https://cms.sklep-a.pl --site sklep   # profil „sklep-a”
nexo login --url https://cms.firma-b.pl --site firma   # profil „firma-b”
nexo profile use sklep-a
nexo -p firma-b docs list
```

## Przykłady

```bash
nexo status
nexo docs list -k page
nexo docs show /kontakt
nexo docs export /kontakt -o kontakt.json   # edytujesz plik…
nexo docs import /kontakt kontakt.json --publish
nexo docs create "Nowa strona" --parent /o-nas
nexo media upload ./zdjecia/ --alt "Salon w stylu boho"
nexo menus show header
nexo terms add "Japandi" -t styl
nexo forms submissions kontakt --status new
nexo forms export kontakt -o zgloszenia.csv
nexo redirects add /stary-adres /nowy-adres
nexo extensions enable com.nexo-cms.commerce
nexo shop orders --status paid
nexo shop status <numer zamówienia> shipped
nexo users invite jan@firma.pl --name "Jan Kowalski" --group Redaktorzy
nexo audit --user jan@firma.pl
```

Każda komenda listująca ma `--json` (np. do `jq`). `nexo <komenda> --help`
pokazuje wszystkie opcje.

## Import i eksport

```bash
# Produkty: wiersz na wariant (SKU, cena, promocja, stan, aktywny).
# CSV otwiera się w Excelu; po edycji wczytujesz go z powrotem.
nexo shop export -o produkty.csv
nexo shop import produkty.csv --dry-run   # pokazuje zmiany, nic nie zapisuje
nexo shop import produkty.csv             # ceny i stany działają od razu
```

W imporcie liczą się tylko wypełnione komórki (pusta = bez zmian, `-` czyści
np. cenę promocyjną). Wystarczą kolumny `sku` i to, co zmieniasz, np.
`sku;price;stock`.

```bash
# Zdjęcia: do jednego produktu albo cały folder po kodzie z nazwy pliku
nexo shop photos add MON001 front.jpg bok.jpg --first --publish
nexo shop photos import ./zdjecia --dry-run   # MON001.jpg, MON001_2.jpg → produkt MON001
nexo shop photos import ./zdjecia --publish

nexo redirects import stare-adresy.csv   # from,to[,code]; pełne adresy starej strony też
nexo redirects export -o przekierowania.csv
nexo terms import kategorie.csv -t product_category   # name,slug,parent,description
nexo collections import handlowcy dane.csv            # kolumny = pola kolekcji
nexo forms export kontakt -o zgloszenia.csv
nexo docs export /o-nas -o o-nas.json && nexo docs import /o-nas o-nas.json --publish
nexo media upload ./folder/
```

## Zdjęcia produktów (AI)

Z prawdziwego zdjęcia produktu Gemini tworzy aranżację wnętrza
(`lifestyle-room`) i ciepły packshot (`warm-packshot`); mebel zostaje taki sam.

```bash
nexo images setup                               # klucz Gemini API (aistudio.google.com/apikey)
nexo images generate MON001 --dry-run           # co by zrobił i z których zdjęć
nexo images generate MON001 --source LET73G_02,LET73G_03   # źródła wskazane ręcznie
nexo images generate MON001 LUM005              # zapis do ./nexo-images/<produkt>/
nexo images generate MON001 --upload --replace  # do biblioteki i galerii produktu (szkic)
nexo images generate MON001 --upload --replace --publish
```

Wbudowane prompty są ogólne. Własne (dopracowane pod sklep) trzymasz w CMS
swojej witryny: zapisujesz je raz, a każdy komputer zalogowany do tego CMS
używa ich sam. Leżą w niepublicznej kolekcji „Prompty zdjęć AI” (edytowalnej
też w panelu); witryna ich nie pokazuje, nie ma ich w tym repozytorium.

```bash
nexo images prompts push --from ./moje-prompty   # lifestyle_room.md, warm_packshot.md → CMS
nexo images prompts list                          # skąd preset bierze prompt
nexo images prompts pull                          # z CMS na ten komputer, do edycji
nexo images prompts set warm-packshot test.md     # tylko na tym komputerze (ma pierwszeństwo)
```

Wyniki mają w nazwie `__cr-<preset>-<hash>`, więc nie są używane jako źródło
kolejnych generacji, a ponowne uruchomienie nie generuje drugi raz tego samego.

## Dla zaawansowanych

```bash
nexo ops menu                       # operacje API tej instalacji
nexo call listRenderers             # dowolna operacja po nazwie
nexo api GET /documents -q site=site_… -q kind=page
```
