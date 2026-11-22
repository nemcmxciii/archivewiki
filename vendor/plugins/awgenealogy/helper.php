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
    const KEYS = ['name', 'sex', 'born', 'birthplace', 'died', 'deathplace', 'father', 'mother', 'living'];

    /** fields the chart form may change */
    const EDITABLE = ['name', 'born', 'birthplace', 'died', 'deathplace'];

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
            "===== Life story =====\n\n===== Timeline =====\n\n===== Sources =====\n\nAdd one source per entry. Include the record or publication, date, archive or URL, and any page or certificate number.\n\n" .
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

    private function familyInput($name, $label, $value = '', $placeholder = '')
    {
        return '<label class="awg-family-field"><span>' . hsc($label) . '</span><input type="text" name="' .
            hsc($name) . '" value="' . hsc($value) . '" placeholder="' . hsc($placeholder) . '" /></label>';
    }

    private function familyTextarea($name, $label, $value = '', $placeholder = '')
    {
        $value = str_replace('\n', "\n", $value);
        return '<label class="awg-family-field"><span>' . hsc($label) . '</span><textarea name="' .
            hsc($name) . '" rows="4" placeholder="' . hsc($placeholder) . '">' . hsc($value) . '</textarea></label>';
    }

    private function familyMemberInput($role, $label, $ref, $fromId)
    {
        $id = $this->resolveRef($ref, $fromId);
        $person = $id !== '' ? $this->getPerson($id) : null;
        $f = $person['fields'] ?? [];
        $html = '<fieldset class="awg-family-member"><legend>' . hsc($label) . '</legend>' .
            '<input type="hidden" name="' . $role . '[id]" value="' . hsc($id) . '" />' .
            $this->familyInput($role . '[name]', 'Full name', $f['name'] ?? '', 'Name') .
            '<div class="awg-pair">' .
            $this->familyInput($role . '[born]', 'Born', $f['born'] ?? '', 'Year or date') .
            $this->familyInput($role . '[birthplace]', 'Birthplace', $f['birthplace'] ?? '') . '</div>' .
            '<div class="awg-pair">' .
            $this->familyInput($role . '[died]', 'Died', $f['died'] ?? '', 'Year or date') .
            $this->familyInput($role . '[deathplace]', 'Place of death', $f['deathplace'] ?? '') . '</div></fieldset>';
        return $html;
    }

    private function familyChildInput($index, $ref, $fromId)
    {
        $id = $this->resolveRef($ref, $fromId);
        $person = $id !== '' ? $this->getPerson($id) : null;
        $f = $person['fields'] ?? [];
        $html = '<div class="awg-child-row"><input type="hidden" name="children[' . $index . '][id]" value="' . hsc($id) . '" />' .
            '<div class="awg-pair">' .
            $this->familyInput('children[' . $index . '][name]', 'Child’s full name', $f['name'] ?? '', 'Name') .
            $this->familyInput('children[' . $index . '][sex]', 'Sex (optional)', $f['sex'] ?? '', 'For example, F or M') . '</div>' .
            '<div class="awg-pair">' .
            $this->familyInput('children[' . $index . '][born]', 'Born', $f['born'] ?? '', 'Year or date') .
            $this->familyInput('children[' . $index . '][birthplace]', 'Birthplace', $f['birthplace'] ?? '') . '</div>' .
            '<div class="awg-pair">' .
            $this->familyInput('children[' . $index . '][died]', 'Died', $f['died'] ?? '', 'Year or date') .
            $this->familyInput('children[' . $index . '][deathplace]', 'Place of death', $f['deathplace'] ?? '') . '</div>' .
            '<button type="button" class="button awg-remove-child">Remove child</button></div>';
        return $html;
    }

    public function familyFormHtml($pageId, $isNew = false)
    {
        $block = $isNew ? null : $this->findFamilyBlock(rawWiki($pageId));
        $f = $block['fields'] ?? [];
        $html = '<div class="awg-family-editor"><p class="awg-family-intro">Enter the parents, marriage or partnership, and children for one family. You can save what you know now and return to it later.</p>' .
            '<form class="awg-family-form" method="post" action="#">' .
            '<input type="hidden" name="call" value="awgenealogy" />' .
            '<input type="hidden" name="mode" value="' . ($isNew ? 'newfamily' : 'savefamily') . '" />' .
            '<input type="hidden" name="id" value="' . hsc($pageId) . '" />' .
            '<input type="hidden" name="sectok" value="' . hsc(getSecurityToken()) . '" />' .
            '<div class="awg-family-title">' . $this->familyInput('title', 'Family group name', $f['title'] ?? '', 'For example, Doe family') . '</div>' .
            '<div class="awg-family-parents">' .
            $this->familyMemberInput('father', 'Father / parent 1', $f['father'] ?? '', $pageId) .
            $this->familyMemberInput('mother', 'Mother / parent 2', $f['mother'] ?? '', $pageId) . '</div>' .
            '<fieldset class="awg-family-marriage"><legend>Marriage or partnership (optional)</legend><div class="awg-pair">' .
            $this->familyInput('marriage_date', 'Date', $f['marriage_date'] ?? '', 'Year or date') .
            $this->familyInput('marriage_place', 'Place', $f['marriage_place'] ?? '') . '</div></fieldset>' .
            '<fieldset class="awg-family-children"><legend>Children</legend><div class="awg-child-rows">';
        $children = [];
        foreach ($f as $key => $value) {
            if (preg_match('/^child_([0-9]+)$/', $key, $m)) $children[(int)$m[1]] = $value;
        }
        if (!$children) $children = [1 => ''];
        ksort($children);
        $index = 0;
        foreach ($children as $ref) $html .= $this->familyChildInput($index++, $ref, $pageId);
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
                '<a href="#" class="awg-add">+ add ' . $label . '</a></div>';
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
        $html .= $in('name', 'Full name', $f['name'] ?? '', 'awg-f-name');
        $html .= '<div class="awg-pair">' . $in('born', 'Born', $f['born'] ?? '') . $in('birthplace', 'Birthplace', $f['birthplace'] ?? '') . '</div>';
        $html .= '<div class="awg-pair">' . $in('died', 'Died', $f['died'] ?? '') . $in('deathplace', 'Place of death', $f['deathplace'] ?? '') . '</div>';
        $html .= '<div class="awg-foot"><label class="awg-living"><input type="hidden" name="' . $name . '[living]" value="0"' . $dis . ' />' .
            '<input type="checkbox" name="' . $name . '[living]" value="1"' . ($living ? ' checked="checked"' : '') . ($canEdit ? '' : ' disabled="disabled"') . $dis . ' /> Living</label>';
        if ($id !== '') $html .= ' <a class="awg-open" href="' . wl($id) . '">Open page</a>';
        elseif ($n > 1) $html .= ' <span class="awg-hint">' . ($blocked ? 'Fill in the previous generation first.' : 'Not created yet') . '</span>';
        return $html . '</div></fieldset>';
    }

    public function viewSwitch($pageId, $current, array $extra = [])
    {
        $link = function ($label, $view) use ($pageId, $current, $extra) {
            if ($view === $current) return '<b>' . $label . '</b>';
            return '<a href="' . wl($pageId, $extra + ['awgview' => $view], false, '&amp;') . '">' . $label . '</a>';
        };
        return '<div class="awg-views">' . $link('Form', 'form') . ' · ' . $link('Tree', 'tree') . '</div>';
    }

    public function formHtml($rootId, $gens, $pageId, array $extra = [])
    {
        $map = $this->ancestors($rootId, $gens);
        if ($map === null) {
            return '<p class="awg-note">Person page <code>' . hsc($rootId) . '</code> doesn\'t exist yet.</p>';
        }
        $html = '<div class="awg-formwrap" data-root="' . hsc($rootId) . '" data-gens="' . (int)$gens . '" data-sectok="' . hsc(getSecurityToken()) . '">';
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
        $people = $this->allPeople();
        if (!$people) {
            return '<p class="awg-note">No people yet. Create a page in the <code>' . hsc($this->peopleNamespace()) .
                '</code> section, for example <code>' . hsc($this->peopleNamespace()) . ':jane_doe</code>.</p>';
        }
        $html = '<form class="awg-picker" method="get" action="' . script() . '">' .
            '<input type="hidden" name="id" value="' . hsc($pageId) . '" />' .
            '<label>Show the chart for <select name="root">';
        foreach ($people as $id => $name) $html .= '<option value="' . hsc($id) . '">' . hsc($name) . '</option>';
        return $html . '</select></label> <button type="submit">Show chart</button></form>';
    }
}
