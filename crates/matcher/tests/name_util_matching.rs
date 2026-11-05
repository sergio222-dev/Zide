//! Port of IntelliJ's `NameUtilMatchingTest` (platform/util/text-matching/test).
//!
//! Like the original, matchers are wrapped by the keyboard-layout fixing matcher.
//! The Pinyin wrapper is not ported yet.

use zide_matcher::{
    AllOccurrencesMatcher, FixingLayoutMatcher, MatchedFragment, Matcher, MatchingMode, NoopLayout,
    RussianToEnglish,
};

fn matcher(pattern: &str, mode: MatchingMode) -> FixingLayoutMatcher {
    FixingLayoutMatcher::new(pattern, mode, "", &RussianToEnglish)
}

fn case_sensitive(pattern: &str) -> FixingLayoutMatcher {
    matcher(pattern, MatchingMode::MatchCase)
}

fn case_insensitive(pattern: &str) -> FixingLayoutMatcher {
    matcher(pattern, MatchingMode::IgnoreCase)
}

fn first_letter(pattern: &str) -> FixingLayoutMatcher {
    matcher(pattern, MatchingMode::FirstLetter)
}

fn frags(pairs: &[(usize, usize)]) -> Vec<MatchedFragment> {
    pairs
        .iter()
        .map(|&(s, e)| MatchedFragment::new(s, e))
        .collect()
}

fn assert_matches(pattern: &str, name: &str) {
    assert!(
        case_insensitive(pattern).matches(name),
        "{pattern} doesn't match {name}!!!"
    );
}

fn assert_doesnt_match(pattern: &str, name: &str) {
    assert!(
        !case_insensitive(pattern).matches(name),
        "{pattern} matches {name}!!!"
    );
}

fn assert_preference(pattern: &str, less: &str, more: &str) {
    assert_preference_mode(pattern, less, more, MatchingMode::FirstLetter);
}

fn assert_preference_mode(pattern: &str, less: &str, more: &str, mode: MatchingMode) {
    let m = matcher(pattern, mode);
    let (i_less, i_more) = (m.matching_degree(less), m.matching_degree(more));
    assert!(i_less < i_more, "{i_less}>={i_more}; {less}>={more}");
}

fn assert_no_preference(pattern: &str, name1: &str, name2: &str, mode: MatchingMode) {
    let m = matcher(pattern, mode);
    assert_eq!(m.matching_degree(name1), m.matching_degree(name2));
}

#[test]
fn long() {
    assert_matches(
        "Product.findByDateAndNameGreaterThanEqualsAndQualityGreaterThanEqual",
        "Product.findByDateAndNameGreaterThanEqualsAndQualityGreaterThanEqualsIntellijIdeaRulezzz",
    );
}

#[test]
fn long_with_space_matches_itself() {
    let s = "the class with its attributes mapped to fields of records parsed by an {@link AbstractParser} or written by an {@link AbstractWriter}.";
    let fragments = case_insensitive(s).match_fragments(s);
    assert_eq!(fragments, Some(frags(&[(0, s.chars().count())])));
}

#[test]
fn matching_fragments() {
    let sample = "NoClassDefFoundException";
    //            0 2    7  10   15    21
    assert_eq!(
        case_insensitive("ncldfou*ion").match_fragments(sample),
        Some(frags(&[(0, 1), (2, 4), (7, 8), (10, 13), (21, 24)]))
    );

    let sample = "doGet(HttpServletRequest, HttpServletResponse):void";
    //            0                     22
    assert_eq!(
        case_insensitive("d*st").match_fragments(sample),
        Some(frags(&[(0, 1), (22, 24)]))
    );
    assert_eq!(
        case_insensitive("doge*st").match_fragments(sample),
        Some(frags(&[(0, 4), (22, 24)]))
    );

    let sample = "_test";
    assert_eq!(
        case_insensitive("_").match_fragments(sample),
        Some(frags(&[(0, 1)]))
    );
    assert_eq!(
        case_insensitive("_t").match_fragments(sample),
        Some(frags(&[(0, 2)]))
    );
}

#[test]
fn matching_fragments_sorted() {
    //   0        9  12
    assert_eq!(
        case_insensitive("SWU*H*R").match_fragments("SWUPGRADEHDLRFSPR7TEST"),
        Some(frags(&[(0, 3), (9, 10), (12, 13)]))
    );
}

#[test]
fn prefer_caps_matching() {
    //   0   4     10
    assert_eq!(
        case_insensitive("getCU").match_fragments("getCurrentUser"),
        Some(frags(&[(0, 4), (10, 11)]))
    );
}

#[test]
fn honor_first_letter_case_in_completion() {
    let m = matcher("*pim", MatchingMode::IgnoreCase);
    let i_less = m.matching_degree_with_case("PImageDecoder", true);
    let i_more = m.matching_degree_with_case("posIdMap", true);
    assert!(i_less < i_more);
}

#[test]
fn meaningful_matching_degree() {
    assert!(case_insensitive(" EUC-").matching_degree("x-EUC-TW") > i32::MIN);
}

#[test]
fn camel_hump_wins_over_consecutive_case_mismatch() {
    assert_eq!(
        case_insensitive("GEN")
            .match_fragments("GetExtendedName")
            .unwrap()
            .len(),
        3
    );

    assert_preference("GEN", "GetName", "GetExtendedName");
    assert_preference("*GEN", "GetName", "GetExtendedName");
}

#[test]
fn test_simple_cases() {
    assert_matches("N", "NameUtilTest");
    assert_matches("NU", "NameUtilTest");
    assert_matches("NUT", "NameUtilTest");
    assert_matches("NaUT", "NameUtilTest");
    assert_doesnt_match("NeUT", "NameUtilTest");
    assert_doesnt_match("NaUTa", "NameUtilTest");
    assert_matches("NaUtT", "NameUtilTest");
    assert_matches("NaUtT", "NameUtilTest");
    assert_matches("NaUtTe", "NameUtilTest");
    assert_matches("AACl", "AAClass");
    assert_matches("ZZZ", "ZZZZZZZZZZ");
}

#[test]
fn test_empty_prefix() {
    assert_matches("", "");
    assert_matches("", "asdfs");
}

#[test]
fn test_skip_words() {
    assert_matches("nt", "NameUtilTest");
    assert_matches("repl map", "ReplacePathToMacroMap");
    assert_matches("replmap", "ReplacePathToMacroMap");
    assert_matches("CertificateEx", "CertificateEncodingException");
    assert_doesnt_match("ABCD", "AbstractButton.DISABLED_ICON_CHANGED_PROPERTY");

    assert_matches("templipa", "template_impl_template_list_panel");
    assert_matches("templistpa", "template_impl_template_list_panel");
}

#[test]
fn test_simple_cases_with_first_lowercased() {
    assert_matches("N", "nameUtilTest");
    assert_doesnt_match("N", "anameUtilTest");
    assert_matches("NU", "nameUtilTest");
    assert_doesnt_match("NU", "anameUtilTest");
    assert_matches("NUT", "nameUtilTest");
    assert_matches("NaUT", "nameUtilTest");
    assert_doesnt_match("NeUT", "nameUtilTest");
    assert_doesnt_match("NaUTa", "nameUtilTest");
    assert_matches("NaUtT", "nameUtilTest");
    assert_matches("NaUtT", "nameUtilTest");
    assert_matches("NaUtTe", "nameUtilTest");
}

#[test]
fn test_space_delimiters() {
    assert_matches("Na Ut Te", "name util test");
    assert_matches("Na Ut Te", "name Util Test");
    assert_doesnt_match("Na Ut Ta", "name Util Test");
    assert_matches("na ut te", "name util test");

    assert_matches("na ut", "name_util_test");
    assert_matches("na te", "name_util_test");
    assert_doesnt_match("na ti", "name_util_test");

    assert_doesnt_match("alias imple", "alias simple");
    assert_doesnt_match("alias mple", "alias simple");
    assert_doesnt_match("alias nother", "alias another");
}

#[test]
fn test_x_m_l_completion() {
    assert_doesnt_match("N_T", "NameUtilTest");
    assert_matches("ORGS_ACC", "ORGS_POSITION_ACCOUNTABILITY");
    assert_matches("ORGS-ACC", "ORGS-POSITION_ACCOUNTABILITY");
    assert_matches("ORGS.ACC", "ORGS.POSITION_ACCOUNTABILITY");
}

#[test]
fn test_star_false_positive() {
    assert_doesnt_match("ar*l*p", "AbstractResponseHandler");
}

#[test]
fn test_underscore_style() {
    assert_matches("N_U_T", "NAME_UTIL_TEST");
    assert_matches("NUT", "NAME_UTIL_TEST");
    assert_doesnt_match("NUT", "NameutilTest");
}

#[test]
fn test_all_uppercase() {
    assert_matches("NOS", "NetOutputStream");
}

#[test]
fn test_common_file_name_conventions() {
    // See IDEADEV-12310

    assert_matches("BLWN", "base_layout_without_navigation.xhtml");
    assert_matches("BLWN", "base-layout-without-navigation.xhtml");
    assert_matches("FC", "faces-config.xml");
    assert_matches("ARS", "activity_report_summary.jsp");
    assert_matches("AD", "arrow_down.gif");
    assert_matches("VL", "vehicle-listings.css");

    assert_matches("ARS.j", "activity_report_summary.jsp");
    assert_doesnt_match("ARS.j", "activity_report_summary.xml");
    assert_doesnt_match("ARS.j", "activity_report_summary_justsometingwrong.xml");

    assert_matches("foo.goo", "foo.bar.goo");
    assert_doesnt_match("*.ico", "sm.th.iks.concierge");
}

#[test]
fn test_space_for_any_words_in_between() {
    assert_matches("fo bar", "fooBar");
    assert_matches("foo bar", "fooBar");
    assert_matches("foo bar", "fooGooBar");
    assert_matches("foo bar", "fooGoo bar");
    assert_doesnt_match(" b", "fbi");
    assert_doesnt_match(" for", "performAction");
    assert!(case_insensitive(" us").matches("getUsage"));
    assert!(case_insensitive(" us").matches("getMyUsage"));
}

#[test]
fn test_filenames_with_dots_and_spaces() {
    assert_matches("Google Test.html", "Google Test Test.cc.html");
    assert_matches("Google.html", "Google Test Test.cc.html");
    assert_matches("Google .html", "Google Test Test.cc.html");
    assert_matches("Google Test*.html", "Google Test Test.cc.html");
}

#[test]
fn test_start_dot() {
    assert_matches("A*.html", "A.html");
    assert_matches("A*.html", "Abc.html");
    assert_matches("A*.html", "after.html");
    assert_doesnt_match("A*.html", "10_after.html");
}

#[test]
fn test_ideadev15503() {
    assert_matches("AR.jsp", "add_relationship.jsp");
    assert_matches("AR.jsp", "advanced_rule.jsp");
    assert_matches("AR.jsp", "alarm_reduction.jsp");
    assert_matches("AR.jsp", "audiot_report.jsp");
    assert_matches("AR.jsp", "audiot_r.jsp");

    assert_matches("AR.jsp", "alarm_rule_action.jsp");
    assert_matches("AR.jsp", "alarm_rule_admin.jsp");
    assert_matches("AR.jsp", "alarm_rule_administration.jsp");
    assert_matches("AR.jsp", "alarm_rule_controller.jsp");
    assert_matches("AR.jsp", "alarm_rule_frame.jsp");
    assert_matches("AR.jsp", "alarm_rule_severity.jsp");

    assert_matches("AR.jsp", "AddRelationship.jsp");
    assert_matches("AR.jsp", "AdvancedRule.jsp");
    assert_matches("AR.jsp", "AlarmReduction.jsp");
    assert_matches("AR.jsp", "AudiotReport.jsp");
    assert_matches("AR.jsp", "AudiotR.jsp");

    assert_matches("AR.jsp", "AlarmRuleAction.jsp");
    assert_matches("AR.jsp", "AlarmRuleAdmin.jsp");
    assert_matches("AR.jsp", "AlarmRuleAdministration.jsp");
    assert_matches("AR.jsp", "AlarmRuleController.jsp");
    assert_matches("AR.jsp", "AlarmRuleFrame.jsp");
    assert_matches("AR.jsp", "AlarmRuleSeverity.jsp");
}

#[test]
fn test_skip_dot() {
    assert_matches("ja", "jquery.autocomplete.js");
    assert_doesnt_match("ja.js", "jquery.autocomplete.js");
    assert_matches("jajs", "jquery.autocomplete.js");
    assert_matches("jjs", "jquery.autocomplete.js");
    assert_matches("j.js", "jquery.autocomplete.js");
    assert_doesnt_match("j.ajs", "jquery.autocomplete.js");
    assert_matches("oracle.bnf", "oracle-11.2.bnf");
    assert_matches("*foo.*bar", "foo.b.bar");
}

#[test]
fn test_no_extension() {
    assert_matches("#.p", "#.php");
    assert_matches("#", "#.php");
    assert_matches("a", "a.php");
}

#[test]
fn test_ignore_case_when_complete_match() {
    assert_matches("comboBox", "combobox");
    assert_matches("combobox", "comboBox");
}

#[test]
fn test_starts_with_dot() {
    assert_matches(".foo", ".foo");
}

#[test]
fn test_proper_dot_escaping() {
    assert_matches("*inspection*.pro", "InspectionsBundle.properties");
    assert_doesnt_match("*inspection*.pro", "InspectionsInProgress.png");
}

#[test]
fn test_leading_underscore() {
    assert_doesnt_match("form", "_form.html.erb");
    assert_matches("_form", "_form.html.erb");
    assert_matches("_form", "__form");
    assert!(first_letter("_form").matches("__form"));
}

#[test]
fn test_lower_case_words() {
    assert_matches("uct", "unit_controller_test");
    assert_matches("unictest", "unit_controller_test");
    assert_matches("uc", "unit_controller_test");
    assert_doesnt_match("nc", "unit_controller_test");
    assert_doesnt_match("utc", "unit_controller_test");
}

#[test]
fn test_objective_c_cases() {
    assert_matches("h*:", "h:aaa");
    assert_matches("h:", "h:aaa");
    assert_matches(
        "text:sh",
        "textField:shouldChangeCharactersInRange:replacementString:",
    );
    assert_matches("abc", "aaa:bbb:ccc");
    assert_matches(
        "textField:sh",
        "textField:shouldChangeCharactersInRange:replacementString:",
    );
    assert_matches(
        "text*:sh",
        "textField:shouldChangeCharactersInRange:replacementString:",
    );
}

#[test]
fn test_middle_matching_minimum_two_consecutive_letters_in_word_middle() {
    assert_matches("*fo", "reformat");
    assert_matches("*f", "reFormat");
    assert_matches("*f", "format");
    assert_matches("*f", "Format");
    assert_matches("*Stri", "string");
    assert_matches("*f", "reformat");
    assert_matches("*f", "reformatCode");
    assert_doesnt_match("*fc", "reformatCode");
    assert_doesnt_match("*foc", "reformatCode");
    assert_matches("*forc", "reformatCode");
    assert_doesnt_match("*sTC", "LazyClassTypeConstructor");

    assert_doesnt_match("*Icon", "LEADING_CONSTRUCTOR");
    assert_matches("*I", "LEADING_CONSTRUCTOR");
    assert_matches("*i", "LEADING_CONSTRUCTOR");
    assert_matches("*in", "LEADING_CONSTRUCTOR");
    assert_matches("*ing", "LEADING_CONSTRUCTOR");
    assert_doesnt_match("*inc", "LEADING_CONSTRUCTOR");
    assert_doesnt_match("*ico", "drawLinePickedOut");

    assert_matches("*l", "AppDelegate");
    assert_matches("*le", "AppDelegate");
    assert_matches("*leg", "AppDelegate");
}

#[test]
fn test_middle_matching_underscore() {
    assert_matches("*_dark", "collapseAll_dark.png");
    assert_matches("*_dark.png", "collapseAll_dark.png");
    assert_matches("**_dark.png", "collapseAll_dark.png");
    assert!(first_letter("*_DARK").matches("A_DARK.png"));
}

#[test]
fn test_middle_matching() {
    assert_matches("*zz*", "ListConfigzzKey");
    assert_matches("*zz", "ListConfigzzKey");
    assert!(case_insensitive("*old").matches("folder"));
    assert_matches("SWU*H*7", "SWUpgradeHdlrFSPR7Test");
    assert_matches("SWU*H*R", "SWUpgradeHdlrFSPR7Test");
    assert_matches("SWU*H*R", "SWUPGRADEHDLRFSPR7TEST");
    assert_matches("*git", "GitBlaBla");
    assert_matches("*Git", "GitBlaBla");
    assert_doesnt_match("*get*A", "getClass");
    assert_matches("*git", "BlaGitBla");
    assert_matches("*Git", "BlaGitBla");
    assert!(first_letter("*Git").matches("BlagitBla"));
    assert_matches("*git", "BlagitBla");
    assert_matches("*Git*", "AtpGenerationItem");
    assert_matches("Collec*Util*", "CollectionUtils");
    assert_matches("Collec*Util*", "CollectionUtilsTest");
    assert!(case_insensitive("*us").matches("usage"));
    assert!(case_insensitive(" us").matches("usage"));
    assert!(case_insensitive(" fo. ba").matches("getFoo.getBar"));
    assert_matches(" File. sepa", "File.separator");
    assert_matches(" File. sepa", "File._separator");
    assert_matches(" File. _sepa", "File._separator");
    assert_matches(" _fo", "_foo");
    assert_matches("*BComp", "BaseComponent");
}

#[test]
fn test_uppercase_prefix_with_middle_matching() {
    assert_matches("*OS", "ios");
    assert_matches("*OS", "IOS");
    assert_matches("*OS", "osx");
    assert_matches("*OS", "OSX");

    assert!(first_letter("*I").matches("ID"));
    assert!(!first_letter("*I").matches("id"));
}

#[test]
fn test_asterisk_ending_inside_uppercase_word() {
    assert_matches("*LRUMap", "SLRUMap");
}

#[test]
fn test_middle_matching_first_letter_sensitive() {
    assert!(first_letter(" cl").matches("getClass"));
    assert!(!first_letter(" EUC-").matches("x-EUC-TW"));
    assert!(first_letter(" a").matches("aaa"));
    assert!(!first_letter(" a").matches("Aaa"));
    assert!(!first_letter(" a").matches("Aaa"));
    assert!(!first_letter(" _bl").matches("_top"));
    assert!(!first_letter("*Ch").matches("char"));
    assert!(first_letter("*Codes").matches("CFLocaleCopyISOCountryCodes"));
    assert!(!first_letter("*codes").matches("CFLocaleCopyISOCountryCodes"));
    assert!(first_letter("*codes").matches("getCFLocaleCopyISOCountryCodes"));
    assert!(first_letter("*Bcomp").matches("BaseComponent"));
}

#[test]
fn test_prefer_camel_humps_to_all_uppers() {
    assert_preference("ProVi", "PROVIDER", "ProjectView");
}

#[test]
fn test_space_in_completion_prefix() {
    assert!(case_insensitive("create ").matches("create module"));
}

#[test]
fn test_upper_case_matches_lower_case() {
    assert_matches("ABC_B.C", "abc_b.c");
}

#[test]
fn test_lower_case_humps() {
    assert_matches("foo", "foo");
    assert_doesnt_match("foo", "fxoo");
    assert_matches("foo", "fOo");
    assert_matches("foo", "fxOo");
    assert_matches("foo", "fXOo");
    assert_matches("fOo", "foo");
    assert_doesnt_match("fOo", "FaOaOaXXXX");
    assert_matches("ncdfoe", "NoClassDefFoundException");
    assert_matches("fob", "FOO_BAR");
    assert_matches("fo_b", "FOO_BAR");
    assert_matches("fob", "FOO BAR");
    assert_matches("fo b", "FOO BAR");
    assert_matches("AACl", "AAClass");
    assert_matches("ZZZ", "ZZZZZZZZZZ");
    assert_matches("em", "emptyList");
    assert_matches("bui", "BuildConfig.groovy");
    assert_matches("buico", "BuildConfig.groovy");
    assert_matches("buico.gr", "BuildConfig.groovy");
    assert_matches("bui.gr", "BuildConfig.groovy");
    assert_matches("*fz", "azzzfzzz");

    assert_matches("WebLogic", "Weblogic");
    assert_matches("WebLOgic", "WebLogic");
    assert_matches("WEbLogic", "WebLogic");
    assert_doesnt_match("WebLogic", "Webologic");

    assert_matches("Wlo", "WebLogic");
}

#[test]
fn test_final_space() {
    assert_matches("a ", "alpha + beta");
    assert_matches("a ", "a ");
    assert_matches("a ", "a");
    assert_matches("GrDebT ", "GroovyDebuggerTest");
    assert_doesnt_match("grdebT ", "GroovyDebuggerTest");
    assert_doesnt_match("grdebt ", "GroovyDebuggerTest");
    assert_matches("Foo ", "Foo");
    assert_doesnt_match("Foo ", "FooBar");
    assert_doesnt_match("Foo ", "Foox");
    assert_doesnt_match("Collections ", "CollectionSplitter");
    assert_matches("CollectionS ", "CollectionSplitter");
    assert_matches("*run ", "in Runnable.run");

    assert_doesnt_match("*l ", "AppDelegate");
    assert_doesnt_match("*le ", "AppDelegate");
    assert_doesnt_match("*leg ", "AppDelegate");
}

#[test]
fn test_digits() {
    assert_matches("foba4", "FooBar4");
    assert_matches("foba", "Foo4Bar");
    assert_matches("*TEST-* ", "TEST-001");
    assert_matches("*TEST-0* ", "TEST-001");
    assert_matches("*v2 ", "VARCHAR2");
    assert_matches("smart8co", "SmartType18CompletionTest");
    assert_matches("smart8co", "smart18completion");
}

#[test]
fn test_do_not_allow_digits_between_matching_digits() {
    assert_doesnt_match("*012", "001122");
    assert_matches("012", "0a1_22");
}

#[test]
fn test_special_symbols() {
    assert_matches("a@b", "a@bc");
    assert_doesnt_match("*@in", "a int");

    assert_matches("a/text", "a/Text");
    assert_matches("a/text", "a/bbbText");
}

#[test]
fn test_minuscule_first_letter() {
    assert!(first_letter("WebLogic").matches("WebLogic"));
    assert!(!first_letter("webLogic").matches("WebLogic"));
    assert!(first_letter("cL").matches("class"));
    assert!(first_letter("CL").matches("Class"));
    assert!(first_letter("Cl").matches("CoreLoader"));
    assert!(!first_letter("abc").matches("_abc"));
}

#[test]
fn test_minuscule_all_important() {
    assert!(case_sensitive("WebLogic").matches("WebLogic"));
    assert!(!case_sensitive("webLogic").matches("weblogic"));
    assert!(!case_sensitive("FOO").matches("foo"));
    assert!(!case_sensitive("foo").matches("fOO"));
    assert!(!case_sensitive("Wl").matches("WebLogic"));
    assert!(case_sensitive("WL").matches("WebLogic"));
    assert!(!case_sensitive("WL").matches("Weblogic"));
    assert!(!case_sensitive("WL").matches("weblogic"));
    assert!(!case_sensitive("webLogic").matches("WebLogic"));
    assert!(!case_sensitive("Str").matches("SomeThingRidiculous"));
    assert!(!case_sensitive("*list*").matches("List"));
    assert!(!case_sensitive("*list*").matches("AbstractList"));
    assert!(!case_sensitive("java.util.list").matches("java.util.List"));
    assert!(!case_sensitive("java.util.list").matches("java.util.AbstractList"));
}

#[test]
fn test_plus_or_minus_in_the_pattern_should_allow_to_be_space_surrounded() {
    assert_matches("a+b", "alpha+beta");
    assert_matches("a+b", "alpha_gamma+beta");
    assert_matches("a+b", "alpha + beta");
    assert_matches("Foo+", "Foo+Bar.txt");
    assert_matches("Foo+", "Foo + Bar.txt");
    assert_matches("a", "alpha+beta");
    assert_matches("*b", "alpha+beta");
    assert_matches("a + b", "alpha+beta");
    assert_matches("a+", "alpha+beta");
    assert_doesnt_match("a ", "alpha+beta");
    assert_matches("", "alpha+beta");
    assert_matches("*+ b", "alpha+beta");
    assert_doesnt_match("d+g", "alphaDelta+betaGamma");
    assert_matches("*d+g", "alphaDelta+betaGamma");

    assert_matches("a-b", "alpha-beta");
    assert_matches("a-b", "alpha - beta");
}

#[test]
fn test_matching_degree() {
    assert_preference_mode(
        "jscote",
        "JsfCompletionTest",
        "JSCompletionTest",
        MatchingMode::IgnoreCase,
    );
    assert_preference("OCO", "OneCoolObject", "OCObject");
    assert_preference("MUp", "MavenUmlProvider", "MarkUp");
    assert_preference("MUP", "MarkUp", "MavenUmlProvider");
    assert_preference(
        "CertificateExce",
        "CertificateEncodingException",
        "CertificateException",
    );
    assert_preference_mode("boo", "Boolean", "boolean", MatchingMode::IgnoreCase);
    assert_preference_mode("Boo", "boolean", "Boolean", MatchingMode::IgnoreCase);
    assert_preference("getCU", "getCurrentSomething", "getCurrentUser");
    assert_preference("cL", "class", "coreLoader");
    assert_preference("cL", "class", "classLoader");
    assert_preference_mode(
        "inse",
        "InstrumentationError",
        "intSet",
        MatchingMode::IgnoreCase,
    );
    assert_preference_mode("String", "STRING", "String", MatchingMode::IgnoreCase);
    assert_preference_mode("*String", "STRING", "String", MatchingMode::IgnoreCase);
}

#[test]
fn test_prefer_adjacent_words() {
    assert_preference_mode(
        "*psfi",
        "PsiJavaFileBaseImpl",
        "PsiFileImpl",
        MatchingMode::IgnoreCase,
    );
}

#[test]
fn test_prefer_matches_to_the_end() {
    assert_preference_mode("*e", "fileIndex", "file", MatchingMode::IgnoreCase);
}

#[test]
fn test_preferences() {
    assert_preference_mode(" fb", "FooBar", "_fooBar", MatchingMode::IgnoreCase);
    assert_preference("*foo", "barFoo", "foobar");
    assert_preference("*fo", "barfoo", "barFoo");
    assert_preference("*fo", "barfoo", "foo");
    assert_preference_mode("*fo", "asdfo", "Foo", MatchingMode::IgnoreCase);
    assert_preference_mode(
        " sto",
        "StackOverflowError",
        "ArrayStoreException",
        MatchingMode::IgnoreCase,
    );
    assert_preference_mode(" EUC-", "x-EUC-TW", "EUC-JP", MatchingMode::FirstLetter);
    assert_preference_mode(" boo", "Boolean", "boolean", MatchingMode::IgnoreCase);
    assert_preference_mode(" Boo", "boolean", "Boolean", MatchingMode::IgnoreCase);
    assert_preference("ob", "oci_bind_array_by_name", "obj");
    assert_no_preference("en", "ENABLED", "Enum", MatchingMode::IgnoreCase);
}

#[test]
fn test_prefer_word_boundary_match() {
    assert_preference_mode(
        "*ap",
        "add_profile",
        "application",
        MatchingMode::IgnoreCase,
    );
    assert_preference("*les", "configureByFiles", "getLookupElementStrings");
    assert_preference_mode(
        "*les",
        "configureByFiles",
        "getLookupElementStrings",
        MatchingMode::IgnoreCase,
    );
    assert_preference_mode("*ea", "LEADING", "NORTH_EAST", MatchingMode::IgnoreCase);

    assert_preference_mode(
        "*Icon",
        "isControlKeyDown",
        "getErrorIcon",
        MatchingMode::IgnoreCase,
    );
    assert_preference_mode(
        "*icon",
        "isControlKeyDown",
        "getErrorIcon",
        MatchingMode::IgnoreCase,
    );

    assert_preference_mode(
        "*Icon",
        "getInitControl",
        "getErrorIcon",
        MatchingMode::IgnoreCase,
    );
    assert_preference_mode(
        "*icon",
        "getInitControl",
        "getErrorIcon",
        MatchingMode::IgnoreCase,
    );
}

#[test]
fn test_prefer_no_word_skipping() {
    assert_preference_mode(
        "CBP",
        "CustomProcessBP",
        "ComputationBatchProcess",
        MatchingMode::IgnoreCase,
    );
}

#[test]
fn test_word_length_does_not_matter() {
    assert_no_preference(
        "PropComp",
        "PropertyComponent",
        "PropertiesComponent",
        MatchingMode::IgnoreCase,
    );
}

#[test]
fn test_match_start_doesnt_matter_for_degree() {
    assert_no_preference(
        " path",
        "getAbsolutePath",
        "findPath",
        MatchingMode::FirstLetter,
    );
}

#[test]
fn test_prefer_start_matching() {
    assert_preference_mode("*tree", "FooTree", "Tree", MatchingMode::IgnoreCase);
}

#[test]
fn test_prefer_contiguous_matching() {
    assert_preference_mode(
        "*mappablejs",
        "mappable-js.scope.js",
        "MappableJs.js",
        MatchingMode::IgnoreCase,
    );
}

#[test]
fn test_file_patterns() {
    assert_matches("groovy*.jar", "groovy-1.7.jar");
    assert_doesnt_match("*.ico", "a.i.c.o");
}

#[test]
fn test_caps_may_match_non_caps() {
    assert_matches("PDFRe", "PdfRenderer");
    assert_matches(
        "*pGETPartTimePositionInfo",
        "dbo.pGetPartTimePositionInfo.sql",
    );
}

#[test]
fn test_a_capital_after_another_capital_may_match_a_lowercase_letter_because_shift_was_accidentally_held_too_long()
 {
    assert_matches("USerDefa", "UserDefaults");
    assert_matches("NSUSerDefa", "NSUserDefaults");
    assert_matches("NSUSER", "NSUserDefaults");
    assert_matches("NSUSD", "NSUserDefaults");
    assert_matches("NSUserDEF", "NSUserDefaults");
}

#[test]
fn test_lower_case_after_camels() {
    assert_matches("LSTMa", "LineStatusTrackerManager");
}

#[test]
fn test_properties() {
    assert_matches(
        "*pro",
        "spring.activemq.pool.configuration.reconnect-on-exception",
    );
}

#[test]
fn cyrillic_match() {
    assert_matches("ыек", "String");
}

#[test]
fn matching_all_occurrences() {
    let m = AllOccurrencesMatcher::new("*e", MatchingMode::IgnoreCase, "", &NoopLayout);
    assert_eq!(
        m.match_fragments("some text"),
        Some(frags(&[(3, 4), (6, 7)]))
    );
}
