<?php

use dokuwiki\Extension\SyntaxPlugin;

/**
 * <person> ... </person>: shows a person's details as a small card.
 * @license GPL 2 (http://www.gnu.org/licenses/gpl.html)
 */
class syntax_plugin_awgenealogy_person extends SyntaxPlugin
{
    public function getType() { return 'substition'; }
    public function getPType() { return 'block'; }
    public function getSort() { return 160; }

    public function connectTo($mode)
    {
        $this->Lexer->addSpecialPattern('<person>.*?</person>', $mode, 'plugin_awgenealogy_person');
    }

    public function handle($match, $state, $pos, $handler)
    {
        /** @var helper_plugin_awgenealogy $h */
        $h = plugin_load('helper', 'awgenealogy');
        $blk = $h->findBlock($match);
        return $blk ? $blk['fields'] : [];
    }

    public function render($format, $renderer, $data)
    {
        if ($format !== 'xhtml') return true;
        global $ID;
        $renderer->info['cache'] = false; // family-group changes affect this person's page
        /** @var helper_plugin_awgenealogy $h */
        $h = plugin_load('helper', 'awgenealogy');
        $f = $data;
        $rows = '';
        $add = function ($label, $value) use (&$rows) {
            if (trim($value) !== '') $rows .= '<dt>' . $label . '</dt><dd>' . $value . '</dd>';
        };
        $born = trim(($f['born'] ?? '') . (($f['birthplace'] ?? '') !== '' ? ' — ' . $f['birthplace'] : ''));
        $died = trim(($f['died'] ?? '') . (($f['deathplace'] ?? '') !== '' ? ' — ' . $f['deathplace'] : ''));
        $add('Born', hsc($born));
        $add('Died', hsc($died));
        foreach (['father' => 'Father', 'mother' => 'Mother'] as $k => $label) {
            $pid = $h->resolveRef($f[$k] ?? '', $ID);
            if ($pid !== '') {
                $p = $h->getPerson($pid);
                if ($p && !empty($p['private'])) {
                    $add($label, 'Private');
                    continue;
                }
                $name = $p ? ($p['fields']['name'] ?? $h->titleFromId($pid)) : $h->titleFromId($pid);
                $add($label, '<a href="' . wl($pid) . '" class="' . ($p ? 'wikilink1' : 'wikilink2') . '">' . hsc($name) . '</a>');
            }
        }
        if (in_array(strtolower(trim($f['living'] ?? '')), ['yes', 'y', 'true', '1'], true)) $add('Status', 'Living');
        $citations = '';
        foreach ([
            'born_source' => 'Birth date source',
            'birthplace_source' => 'Birthplace source',
            'died_source' => 'Death date source',
            'deathplace_source' => 'Place of death source',
        ] as $key => $label) {
            if (trim($f[$key] ?? '') !== '') {
                $citations .= '<dt>' . hsc($label) . '</dt><dd>' . hsc($f[$key]) . '</dd>';
            }
        }
        $renderer->doc .= '<div class="awg-person"><div class="awg-person-name">' . hsc($f['name'] ?? '') . '</div>' .
            ($rows !== '' ? '<dl>' . $rows . '</dl>' : '') . '</div>' .
            ($citations !== '' ? '<details class="awg-person-citations"><summary>Sources for life events</summary><dl>' . $citations . '</dl></details>' : '') .
            $h->familyConnectionsHtml($ID);
        return true;
    }
}
