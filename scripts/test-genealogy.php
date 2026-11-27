<?php

namespace dokuwiki\Extension {
    class Plugin {}
}

namespace {
    define('AUTH_READ', 1);
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
    function resolve_pageid($namespace, &$page, &$exists)
    {
        if (strpos($page, ':') === false && $namespace !== '') $page = $namespace . ':' . $page;
        $exists = page_exists($page);
    }
    function search(&$data, $dir, $function, $options, $namespace)
    {
        $prefix = str_replace('/', ':', $namespace) . ':';
        foreach (array_keys($GLOBALS['archivewikiTestPages']) as $id) {
            if (strpos($id, $prefix) === 0) $data[] = ['id' => $id];
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

    $GLOBALS['archivewikiTestPages'] = [
        'genealogy:alice' => "<person>\nname: Alice Example\n</person>",
        'genealogy:bob' => "<person>\nname: Bob Example\n</person>",
        'genealogy:child' => "<person>\nname: First Child\n</person>",
        'genealogy:sibling' => "<person>\nname: Second Child\n</person>",
        'genealogy:families:example' => "<family>\ntitle: Example family\nfather: genealogy:alice\nmother: genealogy:bob\nchild_1: genealogy:child\nchild_2: genealogy:sibling\n</family>",
    ];

    $helper = new \helper_plugin_awgenealogy();
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

    $GLOBALS['archivewikiTestAcl']['genealogy:bob'] = 0;
    $private = $helper->familyConnectionsHtml('genealogy:alice');
    requireContains($private, 'Private');
    if (strpos($private, 'Bob Example') !== false) {
        fwrite(STDERR, "Private person name was exposed in family connections.\n");
        exit(1);
    }

    unset($GLOBALS['archivewikiTestPages']['genealogy:families:example']);
    $empty = $helper->familyConnectionsHtml('genealogy:alice');
    requireContains($empty, 'No family groups are linked to this person yet.');
    requireContains($empty, 'Create a family group');

    echo "Genealogy family connection checks passed.\n";
}
