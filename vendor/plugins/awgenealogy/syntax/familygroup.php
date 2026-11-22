<?php

use dokuwiki\Extension\SyntaxPlugin;

/** Form-based family group sheets and a local index of those sheets. */
class syntax_plugin_awgenealogy_familygroup extends SyntaxPlugin
{
    public function getType() { return 'substition'; }
    public function getPType() { return 'block'; }
    public function getSort() { return 160; }

    public function connectTo($mode)
    {
        $this->Lexer->addSpecialPattern('\{\{familygroup>new\}\}', $mode, 'plugin_awgenealogy_familygroup');
        $this->Lexer->addSpecialPattern('\{\{familygroup>\}\}', $mode, 'plugin_awgenealogy_familygroup');
        $this->Lexer->addSpecialPattern('\{\{families>\}\}', $mode, 'plugin_awgenealogy_familygroup');
        $this->Lexer->addSpecialPattern('<family>.*?</family>', $mode, 'plugin_awgenealogy_familygroup');
    }

    public function handle($match, $state, $pos, $handler)
    {
        if (strpos($match, '<family>') === 0) return ['suppress' => true];
        if ($match === '{{families>}}') return ['directory' => true];
        return ['new' => $match === '{{familygroup>new}}'];
    }

    public function render($format, $renderer, $data)
    {
        if ($format !== 'xhtml') return true;
        if (!empty($data['suppress'])) return true;
        global $ID;
        /** @var helper_plugin_awgenealogy $h */
        $h = plugin_load('helper', 'awgenealogy');
        if (!empty($data['directory'])) {
            $families = $h->allFamilies();
            if (!$families) {
                $renderer->doc .= '<p class="awg-family-empty">No family groups yet. Create one using the form above.</p>';
                return true;
            }
            $renderer->doc .= '<div class="awg-family-directory"><label><span>Find a family group</span>' .
                '<input type="search" class="awg-family-search" placeholder="Search names, dates, or places" autocomplete="off" /></label>' .
                '<p class="awg-family-count" role="status">' . count($families) . (count($families) === 1 ? ' family group' : ' family groups') . '</p><ul>';
            foreach ($families as $id => $title) {
                $block = $h->findFamilyBlock(rawWiki($id));
                $fields = $block['fields'] ?? [];
                $members = [];
                foreach (['father', 'mother'] as $role) {
                    $pid = $h->resolveRef($fields[$role] ?? '', $id);
                    if ($pid !== '') {
                        $person = $h->getPerson($pid);
                        $members[] = !empty($person['private']) ? 'Private' : ($person['fields']['name'] ?? $h->titleFromId($pid));
                    }
                }
                foreach ($fields as $key => $ref) {
                    if (!preg_match('/^child_[0-9]+$/', $key)) continue;
                    $pid = $h->resolveRef($ref, $id);
                    $person = $pid !== '' ? $h->getPerson($pid) : null;
                    if ($person) $members[] = !empty($person['private']) ? 'Private' : ($person['fields']['name'] ?? $h->titleFromId($pid));
                }
                $search = implode(' ', array_merge([$title, $fields['marriage_date'] ?? '', $fields['marriage_place'] ?? ''], $members));
                $needle = function_exists('mb_strtolower') ? mb_strtolower($search, 'UTF-8') : strtolower($search);
                $renderer->doc .= '<li data-search="' . hsc($needle) . '"><a href="' . wl($id) . '">' . hsc($title) . '</a>' .
                    ($members ? '<span>' . hsc(implode(' · ', $members)) . '</span>' : '') . '</li>';
            }
            $renderer->doc .= '</ul><p class="awg-family-none" hidden>No family groups match that search.</p></div>';
            return true;
        }

        $isNew = !empty($data['new']);
        if ($isNew && $ID !== 'genealogy:families:start') {
            $renderer->doc .= '<p class="awg-family-empty">Create a family group from the Family Groups page.</p>';
            return true;
        }
        if (!$isNew && !$h->findFamilyBlock(rawWiki($ID))) {
            $renderer->doc .= '<p class="awg-family-empty">This page does not contain a family group yet.</p>';
            return true;
        }
        $renderer->doc .= $h->familyFormHtml($ID, $isNew);
        return true;
    }
}
