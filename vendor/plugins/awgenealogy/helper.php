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
            "\n\n===== Summary =====\n\n===== Timeline =====\n\n===== Sources =====\n\n===== Notes =====\n\n" .
            "===== Five-generation chart =====\n\n{{pedigree>$id}}\n";
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
