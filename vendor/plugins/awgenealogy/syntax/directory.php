<?php

use dokuwiki\Extension\SyntaxPlugin;

/** Searchable local directory of person pages. */
class syntax_plugin_awgenealogy_directory extends SyntaxPlugin
{
    public function getType() { return 'substition'; }
    public function getPType() { return 'block'; }
    public function getSort() { return 160; }

    public function connectTo($mode)
    {
        $this->Lexer->addSpecialPattern('{{people>}}', $mode, 'plugin_awgenealogy_directory');
    }

    public function handle($match, $state, $pos, $handler)
    {
        return true;
    }

    public function render($format, $renderer, $data)
    {
        if ($format !== 'xhtml') return true;
        /** @var helper_plugin_awgenealogy $h */
        $h = plugin_load('helper', 'awgenealogy');
        $people = $h->allPeople();
        if (!$people) {
            $renderer->doc .= '<p class="awg-directory-empty">Your people list is empty. Add a person above to get started.</p>';
            return true;
        }

        $renderer->doc .= '<div class="awg-directory"><label class="awg-directory-search"><span>Find a person</span>' .
            '<input type="search" class="awg-people-search" placeholder="Search names, dates, or places" autocomplete="off" /></label>' .
            '<p class="awg-directory-count" role="status">' . count($people) . (count($people) === 1 ? ' person' : ' people') . '</p>' .
            '<ul class="awg-people-list">';
        foreach ($people as $id => $name) {
            $person = $h->getPerson($id);
            $fields = $person['fields'] ?? [];
            $searchable = implode(' ', [$name, $fields['born'] ?? '', $fields['birthplace'] ?? '', $fields['died'] ?? '', $fields['deathplace'] ?? '']);
            $dates = trim(($fields['born'] ?? '') . (($fields['died'] ?? '') !== '' ? ' – ' . $fields['died'] : ''));
            $places = trim(($fields['birthplace'] ?? '') . (($fields['deathplace'] ?? '') !== '' ? ' · ' . $fields['deathplace'] : ''));
            $needle = function_exists('mb_strtolower') ? mb_strtolower($searchable, 'UTF-8') : strtolower($searchable);
            $renderer->doc .= '<li class="awg-person-row" data-search="' . hsc($needle) . '">' .
                '<a href="' . wl($id) . '">' . hsc($name) . '</a>' .
                ($dates !== '' ? '<span>' . hsc($dates) . '</span>' : '') .
                ($places !== '' ? '<small>' . hsc($places) . '</small>' : '') . '</li>';
        }
        $renderer->doc .= '</ul><p class="awg-directory-none" hidden>No people match that search.</p></div>';
        return true;
    }
}
