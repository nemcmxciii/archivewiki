<?php

namespace dokuwiki\Extension {
    class Plugin {}
    class ActionPlugin {}
    class SyntaxPlugin {}
    class EventHandler {}
    class Event {}
}

namespace {
    define('AUTH_READ', 1);
    define('AUTH_EDIT', 2);
    define('AUTH_CREATE', 4);
    $GLOBALS['archivewikiTestPages'] = [];
    $GLOBALS['archivewikiTestAcl'] = [];
    $conf = ['datadir' => 'unused'];

    function cleanID($id) { return trim((string)$id); }
    function noNS($id) { $parts = explode(':', $id); return end($parts); }
    function getNS($id) { $parts = explode(':', $id); array_pop($parts); return implode(':', $parts); }
    function page_exists($id) { return array_key_exists($id, $GLOBALS['archivewikiTestPages']); }
    function auth_quickaclcheck($id) { return $GLOBALS['archivewikiTestAcl'][$id] ?? 16; }
    function rawWiki($id) { return $GLOBALS['archivewikiTestPages'][$id] ?? ''; }
    function hsc($text) { return htmlspecialchars((string)$text, ENT_QUOTES | ENT_SUBSTITUTE, 'UTF-8'); }
    function wl($id) { return '/doku.php?id=' . rawurlencode($id); }
    function script() { return '/doku.php'; }
    function getSecurityToken() { return 'test-security-token'; }
    function checklock($id) { return false; }
    function saveWikiText($id, $text, $summary) { $GLOBALS['archivewikiTestPages'][$id] = $text; }
    function plugin_load($type, $name) { return $GLOBALS['archivewikiTestHelper'] ?? null; }
    function resolve_pageid($namespace, &$page, &$exists)
    {
        if (strpos($page, ':') === false && $namespace !== '') $page = $namespace . ':' . $page;
        $exists = page_exists($page);
    }
    function search(&$data, $dir, $function, $options, $namespace)
    {
        $prefix = str_replace('/', ':', $namespace) . ':';
        foreach (array_keys($GLOBALS['archivewikiTestPages']) as $id) {
            if (strpos($id, $prefix) !== 0) continue;
            $relative = substr($id, strlen($prefix));
            if (($options['depth'] ?? null) === 0 && strpos($relative, ':') !== false) continue;
            $data[] = ['id' => $id];
        }
    }
    function requireContains($text, $needle)
    {
        if (strpos($text, $needle) === false) {
            fwrite(STDERR, "Expected output to contain: $needle\n");
            exit(1);
        }
    }

    require dirname(__DIR__) . '/vendor/plugins/awgenealogy/helper.php';
    require dirname(__DIR__) . '/vendor/plugins/awgenealogy/action.php';
    require dirname(__DIR__) . '/vendor/plugins/awgenealogy/syntax/person.php';

    $GLOBALS['archivewikiTestPages'] = [
        'genealogy:alice' => "<person>\nname: Alice Example\n</person>",
        'genealogy:bob' => "<person>\nname: Bob Example\n</person>",
        'genealogy:child' => "<person>\nname: First Child\n</person>",
        'genealogy:sibling' => "<person>\nname: Second Child\n</person>",
        'genealogy:legacy' => "====== Legacy Person ======\n\nA manually written person page.",
        'genealogy:families:example' => "<family>\ntitle: Example family\nfather: genealogy:alice\nmother: genealogy:bob\nchild_1: genealogy:child\nchild_2: genealogy:sibling\n</family>",
    ];

    $helper = new \helper_plugin_awgenealogy();
    $GLOBALS['archivewikiTestHelper'] = $helper;
    $parent = $helper->familyConnectionsHtml('genealogy:alice');
    requireContains($parent, 'Example family');
    requireContains($parent, 'Parent or partner');
    requireContains($parent, 'Bob Example');
    requireContains($parent, 'First Child');
    requireContains($parent, 'Second Child');

    $child = $helper->familyConnectionsHtml('genealogy:child');
    requireContains($child, 'Parents');
    requireContains($child, 'Alice Example');
    requireContains($child, 'Bob Example');
    requireContains($child, 'Siblings');
    requireContains($child, 'Second Child');

    $familyForm = $helper->familyFormHtml('genealogy:families:example');
    requireContains($familyForm, 'Find or add a person');
    requireContains($familyForm, '<label class="awg-family-field"><span>Find or add a person</span><input type="text" class="awg-person-input"');
    requireContains($familyForm, 'data-people=');
    requireContains($familyForm, 'name="father[id]" value="genealogy:alice"');
    requireContains($familyForm, 'name="children[0][id]" value="genealogy:child"');
    requireContains($familyForm, 'name="parent_roles[father]"');
    requireContains($familyForm, 'name="children[0][relationship]"');
    requireContains($familyForm, 'name="father[born_source]"');
    requireContains($familyForm, 'name="union_source"');

    $chartForm = $helper->formHtml('genealogy:alice', 2, 'genealogy:chart');
    requireContains($chartForm, 'Find an existing person or enter a new name');
    requireContains($chartForm, 'name="p[2][person_id]"');
    requireContains($chartForm, 'data-people=');
    requireContains($chartForm, 'p[1][born_source]');
    $descendants = $helper->descendantsHtml('genealogy:alice', 4);
    requireContains($descendants, 'First Child');
    requireContains($descendants, 'Biological');
    requireContains($descendants, 'Family group');
    $chartPicker = $helper->pickerHtml('genealogy:chart');
    requireContains($chartPicker, 'Alice Example');
    if (strpos($chartPicker, 'Example family') !== false) {
        fwrite(STDERR, "Family group pages were included in the person picker.\n");
        exit(1);
    }
    if (!isset($helper->allPeople()['genealogy:legacy']) || strpos($chartPicker, 'value="genealogy:legacy"') !== false) {
        fwrite(STDERR, "Legacy person pages were not preserved in the directory or were added to the chart picker.\n");
        exit(1);
    }

    class TestPost
    {
        private $data;
        public function __construct(array $data) { $this->data = $data; }
        public function arr($name) { return $this->data[$name] ?? []; }
        public function str($name) { return is_scalar($this->data[$name] ?? null) ? (string)$this->data[$name] : ''; }
        public function int($name, $default = 0) { return isset($this->data[$name]) ? (int)$this->data[$name] : $default; }
    }
    class TestInput
    {
        public $post;
        public function __construct(array $data) { $this->post = new TestPost($data); }
    }
    $action = new \action_plugin_awgenealogy();
    $saveForm = new \ReflectionMethod($action, 'saveForm');
    $GLOBALS['INPUT'] = new TestInput([
        'gens' => 2,
        'p' => [2 => ['person_id' => 'genealogy:bob', 'name' => 'Bob Example']],
    ]);
    ob_start();
    $saveForm->invoke($action, $helper, 'genealogy:alice');
    $linkedResponse = json_decode(ob_get_clean(), true);
    if (empty($linkedResponse['ok']) || ($linkedResponse['linked'] ?? 0) !== 1) {
        fwrite(STDERR, "Selecting an existing person did not link them to the chart.\n");
        exit(1);
    }
    requireContains(rawWiki('genealogy:alice'), 'father: bob');
    requireContains(rawWiki('genealogy:bob'), 'name: Bob Example');

    $GLOBALS['archivewikiTestPages']['genealogy:alice'] = $helper->setFields(rawWiki('genealogy:alice'), ['father' => '']);
    $GLOBALS['archivewikiTestPages']['genealogy:bob'] = $helper->setFields(rawWiki('genealogy:bob'), ['father' => 'alice']);
    $GLOBALS['INPUT'] = new TestInput([
        'gens' => 2,
        'p' => [2 => ['person_id' => 'genealogy:bob', 'name' => 'Bob Example']],
    ]);
    ob_start();
    $saveForm->invoke($action, $helper, 'genealogy:alice');
    $cycleResponse = json_decode(ob_get_clean(), true);
    if (!empty($cycleResponse['ok']) || strpos(rawWiki('genealogy:alice'), 'father: bob') !== false) {
        fwrite(STDERR, "An ancestor cycle was not rejected before chart changes were saved.\n");
        exit(1);
    }
    $GLOBALS['archivewikiTestPages']['genealogy:bob'] = $helper->setFields(rawWiki('genealogy:bob'), ['father' => '']);

    $GLOBALS['INPUT'] = new TestInput([
        'gens' => 2,
        'p' => [
            2 => ['person_id' => 'genealogy:bob', 'name' => 'Bob Example'],
            3 => ['person_id' => 'genealogy:bob', 'name' => 'Bob Example'],
        ],
    ]);
    ob_start();
    $saveForm->invoke($action, $helper, 'genealogy:alice');
    $duplicateResponse = json_decode(ob_get_clean(), true);
    if (!empty($duplicateResponse['ok']) || strpos(rawWiki('genealogy:alice'), 'father: bob') !== false) {
        fwrite(STDERR, "Duplicate people were not rejected before chart changes were saved.\n");
        exit(1);
    }

    $GLOBALS['INPUT'] = new TestInput([
        'gens' => 2,
        'p' => [1 => [
            'name' => 'Alice Example',
            'born_source' => 'County register, page 12',
            'birthplace_source' => 'Parish book, entry 8',
        ]],
    ]);
    ob_start();
    $saveForm->invoke($action, $helper, 'genealogy:alice');
    $citationResponse = json_decode(ob_get_clean(), true);
    if (empty($citationResponse['ok'])) {
        fwrite(STDERR, "Saving citations from the ancestor form failed.\n");
        exit(1);
    }
    requireContains(rawWiki('genealogy:alice'), 'born_source: County register, page 12');
    requireContains(rawWiki('genealogy:alice'), 'birthplace_source: Parish book, entry 8');
    requireContains($helper->chartHtml('genealogy:alice', 2, false), 'data-born_source="County register, page 12"');

    $GLOBALS['archivewikiTestPages']['genealogy:adopted'] = "<person>\nname: Adopted Child\n</person>";
    $saveFamily = new \ReflectionMethod($action, 'saveFamily');
    $GLOBALS['INPUT'] = new TestInput([
        'title' => 'Adoptive',
        'father' => [
            'id' => 'genealogy:alice', 'name' => 'Alice Example',
            'born_source' => 'County register, page 12', 'birthplace_source' => 'Parish book, entry 8',
        ],
        'parent_roles' => ['father' => 'none'],
        'union_type' => 'co_parenting',
        'union_source' => 'Family record, page 3',
        'children' => [['id' => 'genealogy:adopted', 'name' => 'Adopted Child', 'relationship' => 'adoptive']],
    ]);
    ob_start();
    $saveFamily->invoke($action, $helper, '', true);
    $adoptiveResponse = json_decode(ob_get_clean(), true);
    if (empty($adoptiveResponse['ok'])) {
        fwrite(STDERR, "Saving family relationship metadata failed.\n");
        exit(1);
    }
    $adoptiveFamily = $helper->findFamilyBlock(rawWiki('genealogy:families:Adoptive'));
    if (!$adoptiveFamily || ($adoptiveFamily['fields']['child_1_relationship'] ?? '') !== 'adoptive' ||
        ($adoptiveFamily['fields']['parent1_role'] ?? '') !== 'none') {
        fwrite(STDERR, "Family relationship types or parent chart roles were not persisted.\n");
        exit(1);
    }
    requireContains(rawWiki('genealogy:families:Adoptive'), 'union_source: Family record, page 3');
    $adoptedFields = $helper->findBlock(rawWiki('genealogy:adopted'))['fields'];
    if (trim($adoptedFields['father'] ?? '') !== '' || trim($adoptedFields['mother'] ?? '') !== '') {
        fwrite(STDERR, "A non-biological relationship was incorrectly added to the ancestor chart.\n");
        exit(1);
    }
    $descendants = $helper->descendantsHtml('genealogy:alice', 4);
    requireContains($descendants, 'Adopted Child');
    requireContains($descendants, 'Adoptive');
    $GLOBALS['archivewikiTestPages']['genealogy:alice'] = $helper->setFields(rawWiki('genealogy:alice'), ['living' => 'yes']);
    $GLOBALS['archivewikiTestPages']['genealogy:adopted'] = $helper->setFields(rawWiki('genealogy:adopted'), ['living' => 'yes']);
    $redactedDescendants = $helper->descendantsHtml('genealogy:alice', 4, true);
    requireContains($redactedDescendants, 'Descendants of Living');
    if (strpos($redactedDescendants, 'Alice Example') !== false || strpos($redactedDescendants, 'Adopted Child') !== false) {
        fwrite(STDERR, "Living people were not redacted in the descendants view.\n");
        exit(1);
    }

    $GLOBALS['archivewikiTestPages']['genealogy:mapped'] = "<person>\nname: Role-mapped Child\n</person>";
    $GLOBALS['INPUT'] = new TestInput([
        'title' => 'Rolemapped',
        'father' => [
            'id' => 'genealogy:alice', 'name' => 'Alice Example',
            'born_source' => 'County register, page 12', 'birthplace_source' => 'Parish book, entry 8',
        ],
        'parent_roles' => ['father' => 'mother'],
        'children' => [['id' => 'genealogy:mapped', 'name' => 'Role-mapped Child', 'relationship' => 'biological']],
    ]);
    ob_start();
    $saveFamily->invoke($action, $helper, '', true);
    $mappedResponse = json_decode(ob_get_clean(), true);
    if (empty($mappedResponse['ok'])) {
        fwrite(STDERR, "Saving a parent role mapping failed.\n");
        exit(1);
    }
    $mappedFields = $helper->findBlock(rawWiki('genealogy:mapped'))['fields'];
    if (trim($mappedFields['mother'] ?? '') !== 'alice' || trim($mappedFields['father'] ?? '') !== '') {
        fwrite(STDERR, "A family parent role was not mapped to the configured ancestor-chart role.\n");
        exit(1);
    }

    $GLOBALS['archivewikiTestPages']['genealogy:bob'] = $helper->setFields(rawWiki('genealogy:bob'), ['father' => 'alice']);
    $familyBefore = rawWiki('genealogy:families:example');
    $aliceBefore = rawWiki('genealogy:alice');
    $GLOBALS['INPUT'] = new TestInput([
        'title' => 'Example family',
        'father' => ['id' => 'genealogy:bob', 'name' => 'Bob Example'],
        'children' => [['id' => 'genealogy:alice', 'name' => 'Alice Example']],
    ]);
    $saveFamily = new \ReflectionMethod($action, 'saveFamily');
    ob_start();
    $saveFamily->invoke($action, $helper, 'genealogy:families:example', false);
    $familyCycleResponse = json_decode(ob_get_clean(), true);
    if (!empty($familyCycleResponse['ok']) || rawWiki('genealogy:families:example') !== $familyBefore ||
        rawWiki('genealogy:alice') !== $aliceBefore) {
        fwrite(STDERR, "A family group that creates an ancestry cycle was not rejected safely.\n");
        exit(1);
    }

    $GLOBALS['archivewikiTestAcl']['genealogy:bob'] = 0;
    $private = $helper->familyConnectionsHtml('genealogy:alice');
    requireContains($private, 'Private');
    if (strpos($private, 'Bob Example') !== false) {
        fwrite(STDERR, "Private person name was exposed in family connections.\n");
        exit(1);
    }
    $GLOBALS['archivewikiTestPages']['genealogy:alice'] = $helper->setFields(rawWiki('genealogy:alice'), ['father' => 'bob']);
    $ID = 'genealogy:alice';
    $personSyntax = new \syntax_plugin_awgenealogy_person();
    $personRenderer = new class { public $info = []; public $doc = ''; };
    $personSyntax->render('xhtml', $personRenderer, $helper->findBlock(rawWiki('genealogy:alice'))['fields']);
    requireContains($personRenderer->doc, '<dt>Father</dt><dd>Private</dd>');
    requireContains($personRenderer->doc, 'County register, page 12');
    if (strpos($personRenderer->doc, 'Bob Example') !== false) {
        fwrite(STDERR, "A private parent name was exposed on the person page.\n");
        exit(1);
    }

    unset(
        $GLOBALS['archivewikiTestPages']['genealogy:families:example'],
        $GLOBALS['archivewikiTestPages']['genealogy:families:Adoptive'],
        $GLOBALS['archivewikiTestPages']['genealogy:families:Rolemapped']
    );
    $empty = $helper->familyConnectionsHtml('genealogy:alice');
    requireContains($empty, 'No family groups are linked to this person yet.');
    requireContains($empty, 'Create a family group');

    echo "Genealogy workflow checks passed.\n";
}
