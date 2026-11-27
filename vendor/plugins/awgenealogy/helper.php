<?php

use dokuwiki\Extension\Plugin;

/**
 * ArchiveWiki Genealogy: person data and pedigree logic.
 *
 * A person page holds one block:
 *   <person>
 *   name: Jane Doe
 *   born: 1920-03-04
 *   ...
 *   father: john_doe          (page id; relative ids resolve like DokuWiki links)
 *   </person>
 *
 * @license GPL 2 (http://www.gnu.org/licenses/gpl.html)
 */
class helper_plugin_awgenealogy extends Plugin
{
    /** canonical field order */
    const KEYS = ['name', 'sex', 'born', 'birthplace', 'born_source', 'birthplace_source', 'died', 'deathplace', 'died_source', 'deathplace_source', 'father', 'mother', 'living'];

    /** fields the chart form may change */
    const EDITABLE = ['name', 'born', 'birthplace', 'born_source', 'birthplace_source', 'died', 'deathplace', 'died_source', 'deathplace_source'];

    const CHILD_RELATIONSHIPS = [
        'biological' => 'Biological',
        'adoptive' => 'Adoptive',
        'step' => 'Stepchild',
        'foster' => 'Foster',
        'guardian' => 'Guardian',
        'other' => 'Other or uncertain',
    ];

    const UNION_TYPES = [
        'marriage' => 'Marriage',
        'civil_partnership' => 'Civil partnership',
        'domestic_partnership' => 'Domestic partnership',
        'co_parenting' => 'Co-parenting',
        'other' => 'Other or unknown',
    ];

    /** namespace that holds people */
    public function peopleNamespace()
    {
        return 'genealogy';
    }

    // ------------------------------------------------------------ parsing / writing

    /** First <person> block in $text: ['fields'=>[], 'start'=>byte offset, 'length'=>bytes] or null */
    public function findBlock($text)
    {
        if (!preg_match('/<person>(.*?)<\/person>/s', $text, $m, PREG_OFFSET_CAPTURE)) return null;
        $fields = [];
        foreach (preg_split('/\r?\n/', $m[1][0]) as $line) {
            if (preg_match('/^\s*([A-Za-z_]+)\s*:\s*(.*?)\s*$/', $line, $mm)) {
                $fields[strtolower($mm[1])] = $mm[2];
            }
        }
        return ['fields' => $fields, 'start' => $m[0][1], 'length' => strlen($m[0][0])];
    }

    public function findFamilyBlock($text)
    {
        if (!preg_match('/<family>(.*?)<\/family>/s', $text, $m, PREG_OFFSET_CAPTURE)) return null;
        $fields = [];
        foreach (preg_split('/\r?\n/', $m[1][0]) as $line) {
            if (preg_match('/^\s*([A-Za-z_0-9]+)\s*:\s*(.*?)\s*$/', $line, $mm)) {
                $fields[strtolower($mm[1])] = $mm[2];
            }
        }
        return ['fields' => $fields, 'start' => $m[0][1], 'length' => strlen($m[0][0])];
    }

    public function renderFamilyBlock(array $fields)
    {
        $lines = ['<family>'];
        foreach ($fields as $key => $value) $lines[] = $key . ': ' . $value;
        $lines[] = '</family>';
        return implode("\n", $lines);
    }

    public function setFamilyFields($text, array $fields)
    {
        $block = $this->findFamilyBlock($text);
        $saved = $this->renderFamilyBlock($fields);
        if ($block) {
            $text = substr_replace($text, $saved, $block['start'], $block['length']);
            if (isset($fields['title'])) {
                $title = $fields['title'];
                $text = preg_replace_callback('/^======.*?======[ \t]*$/m', function () use ($title) {
                    return '====== ' . $title . ' ======';
                }, $text, 1);
            }
            return $text;
        }
        if (preg_match('/^={2,6}.*={2,6}[ \t]*\r?\n/m', $text, $m, PREG_OFFSET_CAPTURE)) {
            $pos = $m[0][1] + strlen($m[0][0]);
            return substr($text, 0, $pos) . "\n" . $saved . "\n" . substr($text, $pos);
        }
        return $saved . "\n\n" . $text;
    }

    public function renderBlock(array $fields)
    {
        $lines = ['<person>'];
        foreach (self::KEYS as $k) {
            $lines[] = $k . ': ' . ($fields[$k] ?? '');
        }
        foreach ($fields as $k => $v) {
            if (!in_array($k, self::KEYS, true)) $lines[] = $k . ': ' . $v;
        }
        $lines[] = '</person>';
        return implode("\n", $lines);
    }

    /** Return $text with $changes applied to its person block (block is created after the first heading if missing) */
    public function setFields($text, array $changes)
    {
        $blk = $this->findBlock($text);
        $fields = $blk ? $blk['fields'] : [];
        foreach ($changes as $k => $v) $fields[$k] = $v;
        $block = $this->renderBlock($fields);
        if ($blk) return substr_replace($text, $block, $blk['start'], $blk['length']);
        if (preg_match('/^={2,6}.*={2,6}[ \t]*\r?\n/m', $text, $m, PREG_OFFSET_CAPTURE)) {
            $pos = $m[0][1] + strlen($m[0][0]);
            return substr($text, 0, $pos) . "\n" . $block . "\n" . substr($text, $pos);
        }
        return $block . "\n\n" . $text;
    }

    public function newPersonText($id, $name, array $fields)
    {
        $fields = array_merge(['name' => $name, 'living' => 'yes'], $fields);
        return "====== $name ======\n\n" . $this->renderBlock($fields) .
            "\n\n===== Family and relationships =====\n\nRecord spouses, children, siblings, and other family links here. The ancestor chart below follows parents.\n\n" .
            "===== Life story =====\n\n===== Timeline =====\n\n===== Sources =====\n\nUse the person form to attach a source citation to each birth or death fact. Add other sources here with the record or publication, date, archive or URL, and page or certificate number.\n\n" .
            "===== Research notes =====\n\n===== Ancestor chart =====\n\n{{pedigree>$id}}\n";
    }

    public function newFamilyText(array $fields)
    {
        $title = $fields['title'] ?? 'Family group';
        return "====== $title ======\n\n" .
            "Record one family unit here. The linked names open each person’s page; the pedigree tree uses the parent links.\n\n" .
            $this->renderFamilyBlock($fields) . "\n\n{{familygroup>}}\n";
    }

    /** single-line, tag-free value that can't break the block */
    public function cleanValue($v, $max = 200)
    {
        $v = str_replace(["\r", "\n", "\t"], ' ', (string)$v);
        $v = str_replace(['<', '>'], '', $v);
        $v = trim(preg_replace('/\s+/', ' ', $v));
        return function_exists('mb_substr') ? mb_substr($v, 0, $max) : substr($v, 0, $max);
    }

    // ------------------------------------------------------------ people

    public function titleFromId($id)
    {
        return ucwords(str_replace('_', ' ', noNS($id)));
    }

    /** ['id', 'fields'] for an existing, readable person page; ['private'=>true] if not readable; null if missing */
    public function getPerson($id)
    {
        $id = cleanID($id);
        if ($id === '' || !page_exists($id)) return null;
        if (auth_quickaclcheck($id) < AUTH_READ) return ['id' => $id, 'private' => true, 'fields' => []];
        $blk = $this->findBlock(rawWiki($id));
        $f = $blk ? $blk['fields'] : [];
        if (trim($f['name'] ?? '') === '') $f['name'] = $this->titleFromId($id);
        return ['id' => $id, 'fields' => $f];
    }

    public function resolveRef($ref, $fromId)
    {
        $ref = trim(trim((string)$ref), '[]');
        if ($ref === '') return '';
        $page = $ref;
        $exists = false;
        resolve_pageid(getNS($fromId), $page, $exists);
        return cleanID($page);
    }

    /** Whether making $parentId a parent of $childId would create an ancestry cycle. */
    public function wouldCreateAncestryCycle($parentId, $childId, array $plannedParents = [])
    {
        $parentId = cleanID($parentId);
        $childId = cleanID($childId);
        if ($parentId === '' || $childId === '') return false;

        $pending = [$parentId];
        $visited = [];
        while ($pending) {
            $id = array_pop($pending);
            if ($id === $childId) return true;
            if (isset($visited[$id])) continue;
            $visited[$id] = true;

            foreach ($plannedParents[$id] ?? [] as $plannedParent) {
                if ($plannedParent !== '') $pending[] = $plannedParent;
            }
            $person = $this->getPerson($id);
            if (!$person || !empty($person['private'])) continue;
            foreach (['father', 'mother'] as $role) {
                $ancestor = $this->resolveRef($person['fields'][$role] ?? '', $id);
                if ($ancestor !== '') $pending[] = $ancestor;
            }
        }
        return false;
    }

    /**
     * Ahnentafel map for $gens generations: 1 = root, 2 = father, 3 = mother, 2n = father of n, 2n+1 = mother of n.
     * Each value: ['id', 'person'] or ['empty'=>true, 'child'=>id, 'role'=>'father'|'mother'].
     * Returns null if the root page doesn't exist.
     */
    public function ancestors($rootId, $gens)
    {
        $root = $this->getPerson($rootId);
        if (!$root) return null;
        $out = [1 => ['id' => $root['id'], 'person' => $root]];
        for ($n = 1; $n < (1 << ($gens - 1)); $n++) {
            if (!isset($out[$n]) || isset($out[$n]['empty'])) continue;
            $p = $out[$n];
            foreach ([2 * $n => 'father', 2 * $n + 1 => 'mother'] as $k => $role) {
                $pid = $this->resolveRef($p['person']['fields'][$role] ?? '', $p['id']);
                $person = $pid !== '' ? $this->getPerson($pid) : null;
                $out[$k] = $person ? ['id' => $person['id'], 'person' => $person]
                                   : ['empty' => true, 'child' => $p['id'], 'role' => $role];
            }
        }
        return $out;
    }

    /** id => name for every readable person page in the people namespace */
    public function allPeople()
    {
        global $conf;
        $ns = $this->peopleNamespace();
        $data = [];
        search($data, $conf['datadir'], 'search_allpages', ['depth' => 0, 'skipacl' => false], str_replace(':', '/', $ns));
        $people = [];
        foreach ($data as $item) {
            $id = $item['id'];
            if (noNS($id) === 'start' || noNS($id) === 'chart' || noNS($id) === 'chart_blank') continue;
            $p = $this->getPerson($id);
            if ($p && empty($p['private'])) $people[$id] = $p['fields']['name'];
        }
        asort($people, SORT_NATURAL | SORT_FLAG_CASE);
        return $people;
    }

    private function personPickerData($editableOnly = false)
    {
        $people = [];
        foreach ($this->allPeople() as $id => $name) {
            if ($editableOnly && auth_quickaclcheck($id) < AUTH_EDIT) continue;
            $block = $this->findBlock(rawWiki($id));
            if (!$block) continue;
            $fields = $block['fields'];
            $people[] = [
                'id' => $id,
                'name' => $fields['name'] ?? $name,
                'born' => $fields['born'] ?? '',
                'birthplace' => $fields['birthplace'] ?? '',
                'died' => $fields['died'] ?? '',
                'deathplace' => $fields['deathplace'] ?? '',
                'born_source' => $fields['born_source'] ?? '',
                'birthplace_source' => $fields['birthplace_source'] ?? '',
                'died_source' => $fields['died_source'] ?? '',
                'deathplace_source' => $fields['deathplace_source'] ?? '',
                'sex' => $fields['sex'] ?? '',
            ];
        }
        $json = json_encode($people, JSON_HEX_TAG | JSON_HEX_APOS | JSON_HEX_AMP | JSON_HEX_QUOT | JSON_INVALID_UTF8_SUBSTITUTE);
        return hsc($json === false ? '[]' : $json);
    }

    public function allFamilies()
    {
        global $conf;
        $data = [];
        search($data, $conf['datadir'], 'search_allpages', ['depth' => 0, 'skipacl' => false], 'genealogy/families');
        $families = [];
        foreach ($data as $item) {
            $id = $item['id'];
            if (noNS($id) === 'start' || auth_quickaclcheck($id) < AUTH_READ) continue;
            $block = $this->findFamilyBlock(rawWiki($id));
            if (!$block) continue;
            $families[$id] = $block['fields']['title'] ?? $this->titleFromId($id);
        }
        asort($families, SORT_NATURAL | SORT_FLAG_CASE);
        return $families;
    }

    private function familyPeopleLinks(array $ids)
    {
        $links = [];
        foreach (array_unique($ids) as $id) {
            $person = $this->getPerson($id);
            if ($person && !empty($person['private'])) {
                $links[] = 'Private';
            } elseif (!$person) {
                $links[] = hsc($this->titleFromId($id)) . ' (page not found)';
            } else {
                $name = $person['fields']['name'] ?? $this->titleFromId($id);
                $links[] = '<a href="' . hsc(wl($id)) . '">' . hsc($name) . '</a>';
            }
        }
        return implode(', ', $links);
    }

    public function childRelationshipLabel($value)
    {
        return self::CHILD_RELATIONSHIPS[$value] ?? self::CHILD_RELATIONSHIPS['other'];
    }

    public function unionTypeLabel($value)
    {
        return self::UNION_TYPES[$value] ?? '';
    }

    private function familyChildEntries(array $fields, $familyId)
    {
        $children = [];
        foreach ($fields as $key => $ref) {
            if (!preg_match('/^child_([0-9]+)$/', $key, $matches)) continue;
            $id = $this->resolveRef($ref, $familyId);
            if ($id === '') continue;
            $relationship = $fields['child_' . $matches[1] . '_relationship'] ?? 'biological';
            $children[] = [
                'id' => $id,
                'relationship' => $relationship,
                'label' => self::CHILD_RELATIONSHIPS[$relationship] ?? self::CHILD_RELATIONSHIPS['other'],
            ];
        }
        return array_values($children);
    }

    private function familyChildLinks(array $children)
    {
        $links = [];
        foreach ($children as $child) {
            $person = $this->getPerson($child['id']);
            if ($person && !empty($person['private'])) {
                $link = 'Private';
            } elseif (!$person) {
                $link = hsc($this->titleFromId($child['id'])) . ' (page not found)';
            } else {
                $name = $person['fields']['name'] ?? $this->titleFromId($child['id']);
                $link = '<a href="' . hsc(wl($child['id'])) . '">' . hsc($name) . '</a>';
            }
            $links[] = $link . ' <span class="awg-family-relationship">' . hsc($child['label']) . '</span>';
        }
        return implode(', ', $links);
    }

    /** Render family-group connections for a person without requiring edits to the person page. */
    public function familyConnectionsHtml($personId)
    {
        $personId = cleanID($personId);
        $families = [];
        foreach ($this->allFamilies() as $familyId => $title) {
            $block = $this->findFamilyBlock(rawWiki($familyId));
            if (!$block) continue;
            $fields = $block['fields'];
            $parents = [];
            foreach (['father', 'mother'] as $role) {
                $id = $this->resolveRef($fields[$role] ?? '', $familyId);
                if ($id !== '') $parents[] = $id;
            }
            $childEntries = $this->familyChildEntries($fields, $familyId);
            $children = array_column($childEntries, 'id');
            $relationship = '';
            foreach ($childEntries as $childEntry) {
                if ($childEntry['id'] === $personId) {
                    $relationship = $childEntry['label'];
                    break;
                }
            }
            $parents = array_values(array_unique($parents));
            $children = array_values(array_unique($children));
            $isParent = in_array($personId, $parents, true);
            $isChild = in_array($personId, $children, true);
            if (!$isParent && !$isChild) continue;
            $families[] = [
                'id' => $familyId,
                'title' => $title,
                'is_parent' => $isParent,
                'is_child' => $isChild,
                'other_parents' => array_values(array_diff($parents, [$personId])),
                'parents' => $parents,
                'children' => $children,
                'child_entries' => $childEntries,
                'siblings' => array_values(array_diff($children, [$personId])),
                'relationship' => $relationship,
                'union_type' => $fields['union_type'] ?? '',
                'union_source' => $fields['union_source'] ?? '',
                'union_date' => $fields['marriage_date'] ?? '',
                'union_place' => $fields['marriage_place'] ?? '',
            ];
        }

        $html = $this->genealogyNavHtml('people') . '<section class="awg-family-connections"><h3>Family connections</h3>';
        if (!$families) {
            $html .= '<p>No family groups are linked to this person yet. <a href="' .
                hsc(wl('genealogy:families:start')) . '">Create a family group</a> to add parents, partners, or children.</p></section>';
            return $html;
        }

        $html .= '<ul class="awg-family-connections-list">';
        foreach ($families as $family) {
            $html .= '<li><a class="awg-family-title" href="' . hsc(wl($family['id'])) . '">' .
                hsc($family['title']) . '</a><p class="awg-family-role">';
            $roles = [];
            if ($family['is_parent']) $roles[] = 'Parent or partner';
            if ($family['is_child']) $roles[] = 'Child';
            $html .= hsc(implode(' · ', $roles)) . '</p><dl>';
            $unionType = $this->unionTypeLabel($family['union_type']);
            if ($unionType !== '') {
                $unionDetails = array_filter([$unionType, $family['union_date'], $family['union_place']]);
                $html .= '<dt>Partnership</dt><dd>' . hsc(implode(' · ', $unionDetails)) . '</dd>';
            }
            if ($family['union_source'] !== '') $html .= '<dt>Partnership source</dt><dd>' . hsc($family['union_source']) . '</dd>';
            if ($family['is_parent'] && $family['other_parents']) {
                $html .= '<dt>Other parent or partner</dt><dd>' . $this->familyPeopleLinks($family['other_parents']) . '</dd>';
            }
            if ($family['is_parent'] && $family['children']) {
                $html .= '<dt>Children</dt><dd>' . $this->familyChildLinks($family['child_entries']) . '</dd>';
            }
            if ($family['is_child'] && $family['parents']) {
                $html .= '<dt>Parents</dt><dd>' . $this->familyPeopleLinks($family['parents']) . '</dd>';
                if ($family['relationship'] !== '') $html .= '<dt>Relationship</dt><dd>' . hsc($family['relationship']) . '</dd>';
            }
            if ($family['is_child'] && $family['siblings']) {
                $html .= '<dt>Siblings</dt><dd>' . $this->familyPeopleLinks($family['siblings']) . '</dd>';
            }
            $html .= '</dl></li>';
        }
        return $html . '</ul></section>';
    }

    public function descendantsHtml($rootId, $maxGenerations = 5, $redact = false)
    {
        $rootId = cleanID($rootId);
        $root = $this->getPerson($rootId);
        if (!$root) return '<p class="awg-note">That person page does not exist.</p>';
        if (!empty($root['private'])) return '<section class="awg-descendants"><p>Private person.</p></section>';

        $maxGenerations = max(2, min(6, (int)$maxGenerations));
        $familiesByParent = [];
        $biologicalChildren = [];
        foreach ($this->allFamilies() as $familyId => $title) {
            $block = $this->findFamilyBlock(rawWiki($familyId));
            if (!$block) continue;
            $fields = $block['fields'];
            $parents = [];
            foreach (['father', 'mother'] as $role) {
                $id = $this->resolveRef($fields[$role] ?? '', $familyId);
                if ($id !== '') $parents[] = $id;
            }
            $parents = array_values(array_unique($parents));
            $children = $this->familyChildEntries($fields, $familyId);
            if (!$parents || !$children) continue;
            $family = [
                'id' => $familyId,
                'parents' => $parents,
                'children' => $children,
                'union_type' => $this->unionTypeLabel($fields['union_type'] ?? ''),
                'date' => $fields['marriage_date'] ?? '',
                'place' => $fields['marriage_place'] ?? '',
            ];
            foreach ($parents as $parentId) {
                $familiesByParent[$parentId][] = $family;
                foreach ($children as $child) {
                    if ($child['relationship'] === 'biological') {
                        $biologicalChildren[$parentId][$child['id']] = true;
                    }
                }
            }
        }

        $pageChildren = [];
        foreach ($this->allPeople() as $childId => $name) {
            $person = $this->getPerson($childId);
            if (!$person || !empty($person['private'])) continue;
            foreach (['father', 'mother'] as $role) {
                $parentId = $this->resolveRef($person['fields'][$role] ?? '', $childId);
                if ($parentId === '' || isset($biologicalChildren[$parentId][$childId])) continue;
                $pageChildren[$parentId][$childId] = [
                    'id' => $childId,
                    'relationship' => 'biological',
                    'label' => self::CHILD_RELATIONSHIPS['biological'],
                ];
            }
        }

        $personHtml = function ($id) use ($redact) {
            $person = $this->getPerson($id);
            if ($person && !empty($person['private'])) return 'Private';
            if (!$person) return hsc($this->titleFromId($id)) . ' (page not found)';
            $fields = $person['fields'];
            if ($redact && in_array(strtolower(trim($fields['living'] ?? '')), ['yes', 'y', 'true', '1'], true)) {
                return '<span class="awg-desc-redacted">Living</span>';
            }
            return '<a href="' . hsc(wl($id)) . '">' . hsc($fields['name'] ?? $this->titleFromId($id)) . '</a>';
        };

        $nodeCount = 0;
        $renderBranch = function ($parentId, $generation, array $path, array $familyPath = []) use (
            &$renderBranch, &$nodeCount, $maxGenerations, $familiesByParent, $pageChildren, $personHtml
        ) {
            if ($generation >= $maxGenerations || $nodeCount >= 500) return '';
            $html = '';
            $shown = [];
            foreach ($familiesByParent[$parentId] ?? [] as $family) {
                if ($nodeCount >= 500) break;
                if (isset($familyPath[$family['id']])) continue;
                $nextFamilyPath = $familyPath;
                $nextFamilyPath[$family['id']] = true;
                $partners = array_values(array_diff($family['parents'], [$parentId]));
                $html .= '<li class="awg-desc-family"><div class="awg-desc-family-meta"><strong>Family group</strong>';
                if ($partners) $html .= '<span>with ' . implode(', ', array_map($personHtml, $partners)) . '</span>';
                if ($family['union_type'] !== '') $html .= '<span>' . hsc($family['union_type']) . '</span>';
                $dates = trim(implode(' · ', array_filter([$family['date'], $family['place']])));
                if ($dates !== '') $html .= '<span>' . hsc($dates) . '</span>';
                $html .= '</div><ul class="awg-desc-children">';
                foreach ($family['children'] as $child) {
                    if ($nodeCount >= 500) break;
                    $nodeCount++;
                    $id = $child['id'];
                    $person = $this->getPerson($id);
                    $html .= '<li>' . $personHtml($id) . ' <span class="awg-family-relationship">' . hsc($child['label']) . '</span>';
                    if ($person && empty($person['private']) && !isset($path[$id])) {
                        $nextPath = $path;
                        $nextPath[$id] = true;
                        $descendants = $renderBranch($id, $generation + 1, $nextPath, $nextFamilyPath);
                        if ($descendants !== '') $html .= '<ul class="awg-desc-families">' . $descendants . '</ul>';
                    } elseif (isset($path[$id])) {
                        $html .= ' <span class="awg-desc-cycle">Repeated link; further expansion stopped.</span>';
                    }
                    $html .= '</li>';
                }
                $html .= '</ul></li>';
            }
            foreach ($pageChildren[$parentId] ?? [] as $child) {
                if ($nodeCount >= 500) break;
                if (isset($shown[$child['id']])) continue;
                $shown[$child['id']] = true;
                $nodeCount++;
                $id = $child['id'];
                $html .= '<li class="awg-desc-family"><div class="awg-desc-family-meta"><strong>Ancestor chart link</strong></div>' .
                    '<ul class="awg-desc-children"><li>' . $personHtml($id) . ' <span class="awg-family-relationship">' .
                    hsc($child['label']) . '</span>';
                if (!isset($path[$id])) {
                    $nextPath = $path;
                    $nextPath[$id] = true;
                    $descendants = $renderBranch($id, $generation + 1, $nextPath, $familyPath);
                    if ($descendants !== '') $html .= '<ul class="awg-desc-families">' . $descendants . '</ul>';
                }
                $html .= '</li></ul></li>';
            }
            return $html;
        };

        $rootLiving = in_array(strtolower(trim($root['fields']['living'] ?? '')), ['yes', 'y', 'true', '1'], true);
        $rootName = ($redact && $rootLiving) ? 'Living' : ($root['fields']['name'] ?? $this->titleFromId($rootId));
        $path = [$rootId => true];
        $branches = $renderBranch($rootId, 1, $path);
        $html = '<section class="awg-descendants"><h3>Descendants of ' . hsc($rootName) . '</h3>' .
            '<p class="awg-desc-intro">Family Group relationships and ancestor-chart parent links are shown separately. Up to ' .
            $maxGenerations . ' generations are included.</p>';
        if ($branches === '') {
            $html .= '<p>No linked descendants were found. Add children on a Family Group sheet or link parents on a person’s ancestor chart.</p>';
        } else {
            $html .= '<ul class="awg-desc-families">' . $branches . '</ul>';
            if ($nodeCount >= 500) $html .= '<p class="awg-desc-limit">The view reached its safety limit. Narrow the tree or view fewer generations.</p>';
        }
        return $html . '</section>';
    }

    private function familyInput($name, $label, $value = '', $placeholder = '')
    {
        return '<label class="awg-family-field"><span>' . hsc($label) . '</span><input type="text" name="' .
            hsc($name) . '" value="' . hsc($value) . '" placeholder="' . hsc($placeholder) . '" /></label>';
    }

    private function familySelect($name, $label, array $options, $value = '')
    {
        $html = '<label class="awg-family-field"><span>' . hsc($label) . '</span><select name="' . hsc($name) . '">';
        foreach ($options as $option => $text) {
            $selected = $option === $value ? ' selected="selected"' : '';
            $html .= '<option value="' . hsc($option) . '"' . $selected . '>' . hsc($text) . '</option>';
        }
        return $html . '</select></label>';
    }

    public function genealogyNavHtml($current = '')
    {
        $items = [
            'people' => ['genealogy:start', 'People'],
            'families' => ['genealogy:families:start', 'Family Groups'],
            'ancestors' => ['genealogy:chart', 'Ancestor chart'],
        ];
        $html = '<nav class="awg-genealogy-nav" aria-label="Genealogy">';
        foreach ($items as $key => [$id, $label]) {
            if ($key === $current) {
                $html .= '<span aria-current="page">' . hsc($label) . '</span>';
            } else {
                $html .= '<a href="' . hsc(wl($id)) . '">' . hsc($label) . '</a>';
            }
        }
        return $html . '</nav>';
    }

    private function familyTextarea($name, $label, $value = '', $placeholder = '')
    {
        $value = str_replace('\n', "\n", $value);
        return '<label class="awg-family-field"><span>' . hsc($label) . '</span><textarea name="' .
            hsc($name) . '" rows="4" placeholder="' . hsc($placeholder) . '">' . hsc($value) . '</textarea></label>';
    }

    private function citationFields($prefix, array $fields)
    {
        $html = '<details class="awg-fact-sources"><summary>Attach sources to dates and places (optional)</summary>';
        foreach ([
            'born_source' => 'Birth date source',
            'birthplace_source' => 'Birthplace source',
            'died_source' => 'Death date source',
            'deathplace_source' => 'Place of death source',
        ] as $key => $label) {
            $html .= $this->familyInput($prefix . '[' . $key . ']', $label, $fields[$key] ?? '', 'Record, archive, page number, or URL');
        }
        return $html . '</details>';
    }

    private function personLookupHtml($name, $idName, $label, $value, $id, $hint, $linkOnly = false, $disabled = false)
    {
        $disabledAttr = $disabled ? ' disabled="disabled"' : '';
        return '<div class="awg-person-lookup" data-link-only="' . ($linkOnly ? 'true' : 'false') . '">' .
            '<input type="hidden" class="awg-person-id" name="' . hsc($idName) . '" value="' . hsc($id) . '"' . $disabledAttr . ' />' .
            '<div class="awg-person-control"><label class="awg-family-field"><span>' . hsc($label) .
            '</span><input type="text" class="awg-person-input" name="' . hsc($name) . '" value="' . hsc($value) .
            '" autocomplete="off" role="combobox" aria-autocomplete="list" aria-expanded="false"' . $disabledAttr . ' /></label>' .
            '<div class="awg-person-suggestions" role="listbox" hidden></div></div>' .
            '<p class="awg-person-message" role="status">' . hsc($hint) . '</p>' .
            '<button type="button" class="button awg-person-clear" hidden' . $disabledAttr . '>Clear person selection</button></div>';
    }

    private function familyMemberInput($role, $label, $ref, $fromId, $chartRole)
    {
        $id = $this->resolveRef($ref, $fromId);
        $person = $id !== '' ? $this->getPerson($id) : null;
        $f = $person['fields'] ?? [];
        $html = '<fieldset class="awg-family-member"><legend>' . hsc($label) . '</legend>' .
            $this->personLookupHtml(
                $role . '[name]',
                $role . '[id]',
                'Find or add a person',
                $f['name'] ?? '',
                $id,
                'Choose a match to link an existing person. Otherwise, a new person page will be created when you save.'
            ) .
            $this->familySelect(
                'parent_roles[' . $role . ']',
                'How to show this person in ancestor charts',
                ['father' => 'Father', 'mother' => 'Mother', 'none' => 'Family connection only'],
                $chartRole
            ) .
            '<div class="awg-pair">' .
            $this->familyInput($role . '[born]', 'Born', $f['born'] ?? '', 'Year or date') .
            $this->familyInput($role . '[birthplace]', 'Birthplace', $f['birthplace'] ?? '') . '</div>' .
            '<div class="awg-pair">' .
            $this->familyInput($role . '[died]', 'Died', $f['died'] ?? '', 'Year or date') .
            $this->familyInput($role . '[deathplace]', 'Place of death', $f['deathplace'] ?? '') . '</div>' .
            $this->citationFields($role, $f) . '</fieldset>';
        return $html;
    }

    private function familyChildInput($index, $ref, $fromId, $relationship = 'biological')
    {
        $id = $this->resolveRef($ref, $fromId);
        $person = $id !== '' ? $this->getPerson($id) : null;
        $f = $person['fields'] ?? [];
        $html = '<div class="awg-child-row">' .
            $this->personLookupHtml(
                'children[' . $index . '][name]',
                'children[' . $index . '][id]',
                'Find or add a child',
                $f['name'] ?? '',
                $id,
                'Choose an existing person to link them, or enter a name to create a person page when you save.'
            ) . $this->familySelect(
                'children[' . $index . '][relationship]',
                'Child’s relationship to these people',
                self::CHILD_RELATIONSHIPS,
                $relationship
            ) .
            $this->familyInput('children[' . $index . '][sex]', 'Sex (optional)', $f['sex'] ?? '', 'For example, F or M') .
            '<div class="awg-pair">' .
            $this->familyInput('children[' . $index . '][born]', 'Born', $f['born'] ?? '', 'Year or date') .
            $this->familyInput('children[' . $index . '][birthplace]', 'Birthplace', $f['birthplace'] ?? '') . '</div>' .
            '<div class="awg-pair">' .
            $this->familyInput('children[' . $index . '][died]', 'Died', $f['died'] ?? '', 'Year or date') .
            $this->familyInput('children[' . $index . '][deathplace]', 'Place of death', $f['deathplace'] ?? '') . '</div>' .
            $this->citationFields('children[' . $index . ']', $f) .
            '<button type="button" class="button awg-remove-child">Remove child</button></div>';
        return $html;
    }

    public function familyFormHtml($pageId, $isNew = false)
    {
        $block = $isNew ? null : $this->findFamilyBlock(rawWiki($pageId));
        $f = $block['fields'] ?? [];
        $parent1Role = $f['parent1_role'] ?? 'father';
        $parent2Role = $f['parent2_role'] ?? 'mother';
        $html = $this->genealogyNavHtml('families') .
            '<div class="awg-family-editor" data-people="' . $this->personPickerData(true) . '"><p class="awg-family-intro">Enter up to two parents or partners and their children. Choose an existing person when possible; you can add missing details or return to this sheet later.</p>' .
            '<form class="awg-family-form" method="post" action="#">' .
            '<input type="hidden" name="call" value="awgenealogy" />' .
            '<input type="hidden" name="mode" value="' . ($isNew ? 'newfamily' : 'savefamily') . '" />' .
            '<input type="hidden" name="id" value="' . hsc($pageId) . '" />' .
            '<input type="hidden" name="sectok" value="' . hsc(getSecurityToken()) . '" />' .
            '<div class="awg-family-title">' . $this->familyInput('title', 'Family group name', $f['title'] ?? '', 'For example, Doe family') . '</div>' .
            '<div class="awg-family-parents">' .
            $this->familyMemberInput('father', 'Parent or partner 1', $f['father'] ?? '', $pageId, $parent1Role) .
            $this->familyMemberInput('mother', 'Parent or partner 2', $f['mother'] ?? '', $pageId, $parent2Role) . '</div>' .
            '<fieldset class="awg-family-marriage"><legend>Marriage or partnership (optional)</legend>' .
            $this->familySelect('union_type', 'Relationship type', ['' => 'Not specified'] + self::UNION_TYPES, $f['union_type'] ?? '') .
            '<div class="awg-pair">' . $this->familyInput('marriage_date', 'Date', $f['marriage_date'] ?? '', 'Year or date') .
            $this->familyInput('marriage_place', 'Place', $f['marriage_place'] ?? '') . '</div>' .
            $this->familyInput('union_source', 'Relationship source', $f['union_source'] ?? '', 'Record, archive, page number, or URL') .
            '</fieldset>' .
            '<p class="awg-family-hint">Only biological relationships are added to the ancestor chart. Existing father/mother links on person pages are not removed when this family sheet changes.</p>' .
            '<fieldset class="awg-family-children"><legend>Children</legend><div class="awg-child-rows">';
        $children = [];
        foreach ($f as $key => $value) {
            if (preg_match('/^child_([0-9]+)$/', $key, $m)) {
                $number = (int)$m[1];
                $children[$number] = [
                    'ref' => $value,
                    'relationship' => $f['child_' . $number . '_relationship'] ?? 'biological',
                ];
            }
        }
        if (!$children) $children = [1 => ['ref' => '', 'relationship' => 'biological']];
        ksort($children);
        $index = 0;
        foreach ($children as $child) {
            $html .= $this->familyChildInput($index++, $child['ref'], $pageId, $child['relationship']);
        }
        $html .= '</div><button type="button" class="button awg-add-child">+ Add another child (up to 20)</button></fieldset>' .
            '<fieldset class="awg-family-sources"><legend>Sources and notes</legend>' .
            $this->familyTextarea('sources', 'Sources', $f['sources'] ?? '', 'Add one source per line: record, date, archive or URL, and page number') .
            $this->familyTextarea('notes', 'Research notes', $f['notes'] ?? '', 'Uncertainties, clues, or follow-up') . '</fieldset>' .
            '<div class="awg-family-result" role="status" aria-live="polite"></div>' .
            '<div class="awg-family-actions"><button type="submit" class="button">' . ($isNew ? 'Create family group' : 'Save family group') . '</button></div>' .
            '</form></div>';
        return $html;
    }

    // ------------------------------------------------------------ chart HTML

    private function dateLine($label, $date, $place)
    {
        $date = trim($date);
        $place = trim($place);
        if ($date === '' && $place === '') return '';
        return '<div class="awg-line"><span class="awg-k">' . $label . '</span> ' . hsc($date) .
            ($place !== '' ? ($date !== '' ? ', ' : '') . '<span class="awg-place">' . hsc($place) . '</span>' : '') . '</div>';
    }

    private function box($n, $slot, $redact)
    {
        $num = '<span class="awg-num">' . $n . '</span>';
        if ($slot === null) return '<div class="awg-box awg-void">' . $num . '</div>';

        if (isset($slot['empty'])) {
            $child = $slot['child'];
            if (auth_quickaclcheck($child) < AUTH_EDIT) return '<div class="awg-box awg-void">' . $num . '</div>';
            $label = $slot['role'] === 'father' ? 'father' : 'mother';
            return '<div class="awg-box awg-empty" data-child="' . hsc($child) . '" data-role="' . $label . '">' . $num .
                    '<a href="#" class="awg-add">+ Add ' . $label . '</a></div>';
        }

        $id = $slot['id'];
        $p = $slot['person'];
        if (!empty($p['private'])) return '<div class="awg-box awg-private">' . $num . '<span class="awg-name">Private</span></div>';
        $f = $p['fields'];
        $living = in_array(strtolower(trim($f['living'] ?? '')), ['yes', 'y', 'true', '1'], true);
        if ($redact && $living) return '<div class="awg-box awg-living">' . $num . '<span class="awg-name">Living</span></div>';

        $canEdit = auth_quickaclcheck($id) >= AUTH_EDIT;
        $attrs = ' data-id="' . hsc($id) . '"';
        foreach (self::EDITABLE as $k) $attrs .= ' data-' . $k . '="' . hsc($f[$k] ?? '') . '"';
        $html = '<div class="awg-box awg-filled"' . $attrs . '>' . $num .
            '<a class="awg-name" href="' . wl($id) . '">' . hsc($f['name']) . '</a>' .
            $this->dateLine('b.', $f['born'] ?? '', $f['birthplace'] ?? '') .
            $this->dateLine('d.', $f['died'] ?? '', $f['deathplace'] ?? '');
        if ($canEdit) $html .= '<a href="#" class="awg-edit" title="Edit this person\'s details">✎</a>';
        return $html . '</div>';
    }

    /** The generic grid: rows = 2^(gens-1), one column per generation, root on the left */
    private function grid($gens, callable $cellFn)
    {
        $rows = 1 << ($gens - 1);
        $html = '<table class="awg-pedigree awg-gens' . $gens . '"><tbody>';
        for ($r = 0; $r < $rows; $r++) {
            $html .= '<tr>';
            for ($g = 1; $g <= $gens; $g++) {
                $span = 1 << ($gens - $g);
                if ($r % $span !== 0) continue;
                $n = (1 << ($g - 1)) + intdiv($r, $span);
                $html .= '<td rowspan="' . $span . '" class="awg-gen awg-g' . $g . '">' . $cellFn($n) . '</td>';
            }
            $html .= '</tr>';
        }
        return $html . '</tbody></table>';
    }

    public function chartHtml($rootId, $gens, $redact)
    {
        $map = $this->ancestors($rootId, $gens);
        if ($map === null) {
            return '<p class="awg-note">Person page <code>' . hsc($rootId) . '</code> doesn\'t exist yet.</p>';
        }
        $html = '<div class="awg-chart" data-sectok="' . hsc(getSecurityToken()) . '">';
        $html .= $this->grid($gens, function ($n) use ($map, $redact) {
            return $this->box($n, $map[$n] ?? null, $redact);
        });
        return $html . '</div>';
    }

    // ------------------------------------------------------------ form view (vertical, fillable)

    private static $genTitles = [1 => 'Person', 2 => 'Parents', 3 => 'Grandparents', 4 => 'Great-grandparents'];

    public function genTitle($g)
    {
        if (isset(self::$genTitles[$g])) return self::$genTitles[$g];
        return $this->ordinal($g - 3) . ' great-grandparents';
    }

    private function ordinal($i)
    {
        if ($i % 100 >= 11 && $i % 100 <= 13) return $i . 'th';
        $suffix = [1 => 'st', 2 => 'nd', 3 => 'rd'][$i % 10] ?? 'th';
        return $i . $suffix;
    }

    /** ['Father's mother', 'Grandmother', 'Paternal line'] for ahnentafel number $n */
    public function relation($n)
    {
        if ($n === 1) return ['Person', '', ''];
        $g = (int)floor(log($n, 2)) + 1;
        $fem = ($n % 2 === 1);
        if ($g === 2) $title = $fem ? 'Mother' : 'Father';
        elseif ($g === 3) $title = $fem ? 'Grandmother' : 'Grandfather';
        elseif ($g === 4) $title = $fem ? 'Great-grandmother' : 'Great-grandfather';
        else $title = $this->ordinal($g - 3) . ' great-grand' . ($fem ? 'mother' : 'father');
        $bits = substr(decbin($n), 1);
        $path = [];
        foreach (str_split($bits) as $b) $path[] = $b === '0' ? 'father' : 'mother';
        $pathText = $g > 2 ? ucfirst(implode("'s ", $path)) : '';
        $line = $g > 2 ? ($bits[0] === '0' ? 'Paternal line' : 'Maternal line') : '';
        return [$title, $pathText, $line];
    }

    private function card($n, $slot, $map)
    {
        [$title, $path, $line] = $this->relation($n);
        $g = (int)floor(log($n, 2)) + 1;
        $child = intdiv($n, 2);
        $person = (is_array($slot) && isset($slot['person'])) ? $slot['person'] : null;
        $id = $person ? $slot['id'] : '';
        $private = $person && !empty($person['private']);
        $f = ($person && !$private) ? $person['fields'] : [];
        $blocked = ($n > 1 && !$person && !(isset($map[$child]['person'])));
        $canEdit = $person ? (auth_quickaclcheck($id) >= AUTH_EDIT) : true;
        $ro = (!$canEdit || $private) ? ' readonly="readonly"' : '';

        $cls = 'awg-card' . ($blocked ? ' awg-blocked' : '') . ($person ? ' awg-has' : '');
        $dis = ($blocked || $private) ? ' disabled="disabled"' : '';
        $name = 'p[' . $n . ']';
        $in = function ($key, $label, $value, $extra = '') use ($name, $ro, $dis) {
            return '<label class="awg-f ' . $extra . '"><span>' . $label . '</span><input type="text" class="edit" name="' . $name . '[' . $key . ']" value="' . hsc($value) . '"' . $ro . $dis . ' /></label>';
        };
        $living = $person ? in_array(strtolower(trim($f['living'] ?? '')), ['yes', 'y', 'true', '1'], true) : ($g <= 2);

        $html = '<fieldset class="' . $cls . '" data-n="' . $n . '" data-id="' . hsc($id) . '">' .
            '<legend><span class="awg-num">' . $n . '</span> ' . hsc($title) .
            ($path !== '' ? ' <small>' . hsc($path) . '</small>' : '') . '</legend>';
        if ($private) return $html . '<p class="awg-note">Private</p></fieldset>';
        if ($person) {
            $html .= $in('name', 'Full name', $f['name'] ?? '', 'awg-f-name');
        } else {
            $html .= $this->personLookupHtml(
                $name . '[name]',
                $name . '[person_id]',
                'Find an existing person or enter a new name',
                '',
                '',
                'Choose a match to link that person. If there is no match, saving will create a new person page.',
                true,
                $blocked
            );
        }
        $html .= '<div class="awg-pair">' . $in('born', 'Born', $f['born'] ?? '') . $in('birthplace', 'Birthplace', $f['birthplace'] ?? '') . '</div>';
        $html .= '<div class="awg-pair">' . $in('died', 'Died', $f['died'] ?? '') . $in('deathplace', 'Place of death', $f['deathplace'] ?? '') . '</div>';
        $html .= '<details class="awg-fact-sources"><summary>Attach sources to dates and places (optional)</summary>' .
            $in('born_source', 'Birth date source', $f['born_source'] ?? '') .
            $in('birthplace_source', 'Birthplace source', $f['birthplace_source'] ?? '') .
            $in('died_source', 'Death date source', $f['died_source'] ?? '') .
            $in('deathplace_source', 'Place of death source', $f['deathplace_source'] ?? '') . '</details>';
        $html .= '<div class="awg-foot"><label class="awg-living"><input type="hidden" name="' . $name . '[living]" value="0"' . $dis . ' />' .
            '<input type="checkbox" name="' . $name . '[living]" value="1"' . ($living ? ' checked="checked"' : '') . ($canEdit ? '' : ' disabled="disabled"') . $dis . ' /> Living</label>';
        if ($id !== '') $html .= ' <a class="awg-open" href="' . wl($id) . '">Open page</a>';
        elseif ($n > 1) $html .= ' <span class="awg-hint">' . ($blocked ? 'Fill in the previous generation first.' : 'Not created yet') . '</span>';
        return $html . '</div></fieldset>';
    }

    public function viewSwitch($pageId, $current, array $extra = [])
    {
        $link = function ($label, $view) use ($pageId, $current, $extra) {
            if ($view === $current) return '<span aria-current="page">' . hsc($label) . '</span>';
            return '<a href="' . wl($pageId, $extra + ['awgview' => $view], false, '&amp;') . '">' . $label . '</a>';
        };
        $nav = ($pageId === 'genealogy:chart') ? $this->genealogyNavHtml('ancestors') : '';
        return $nav . '<nav class="awg-views" aria-label="Ancestor views">' .
            $link('Form', 'form') . $link('Ancestor tree', 'tree') . $link('Descendants', 'descendants') . '</nav>';
    }

    public function formHtml($rootId, $gens, $pageId, array $extra = [])
    {
        $map = $this->ancestors($rootId, $gens);
        if ($map === null) {
            return '<p class="awg-note">Person page <code>' . hsc($rootId) . '</code> doesn\'t exist yet.</p>';
        }
        $html = '<div class="awg-formwrap" data-root="' . hsc($rootId) . '" data-gens="' . (int)$gens . '" data-sectok="' .
            hsc(getSecurityToken()) . '" data-people="' . $this->personPickerData() . '">';
        $html .= $this->viewSwitch($pageId, 'form', $extra);
        $html .= '<p class="awg-step-intro">Start with the selected person. Add the relatives you know, one generation at a time. You can save now and return later.</p>';
        $html .= '<form class="awg-form" action="#" method="post" autocomplete="off"><div class="awg-msg" role="status" aria-live="polite"></div>';
        for ($g = 1; $g <= $gens; $g++) {
            $html .= '<section class="awg-gensec"><h3>Generation ' . $g . ' · ' . hsc($this->genTitle($g)) . '</h3><div class="awg-cards">';
            for ($n = 1 << ($g - 1); $n < (1 << $g); $n++) $html .= $this->card($n, $map[$n] ?? null, $map);
            $html .= '</div></section>';
        }
        $html .= '<div class="awg-actions"><button type="submit" class="button awg-save" disabled="disabled">Save changes</button> ' .
            '<span class="awg-dirty"></span></div></form></div>';
        return $html;
    }

    public function blankHtml($gens)
    {
        $html = '<div class="awg-chart awg-blank">';
        $html .= $this->grid($gens, function ($n) {
            return '<div class="awg-box awg-blankbox"><span class="awg-num">' . $n . '</span>' .
                '<div class="awg-blankline"></div><div class="awg-blankline awg-b">b.</div><div class="awg-blankline awg-d">d.</div></div>';
        });
        return $html . '</div>';
    }

    public function pickerHtml($pageId)
    {
        $people = [];
        foreach ($this->allPeople() as $id => $name) {
            if ($this->findBlock(rawWiki($id))) $people[$id] = $name;
        }
        if (!$people) {
            return $this->genealogyNavHtml('ancestors') . '<p class="awg-note">No people have been added yet. Start at the <a href="' . hsc(wl('genealogy:start')) .
                '">Genealogy page</a> to add the first person.</p>';
        }
        $html = $this->genealogyNavHtml('ancestors') . '<form class="awg-picker" method="get" action="' . script() . '">' .
            '<input type="hidden" name="id" value="' . hsc($pageId) . '" />' .
            '<label>Show the chart for <select name="root">';
        foreach ($people as $id => $name) $html .= '<option value="' . hsc($id) . '">' . hsc($name) . '</option>';
        return $html . '</select></label> <button type="submit">Show chart</button></form>';
    }
}
