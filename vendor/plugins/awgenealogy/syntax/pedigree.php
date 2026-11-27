<?php

use dokuwiki\Extension\SyntaxPlugin;

/**
 * {{pedigree>page_id?5&redact}}  ancestors: a fillable form (default) or a tree (?tree, or Tree link); redact = tree with living people hidden
 * (root optional: ?root=<id> in the URL, or a picker)
 * {{pedigree>?blank}}            blank chart for printing
 * @license GPL 2 (http://www.gnu.org/licenses/gpl.html)
 */
class syntax_plugin_awgenealogy_pedigree extends SyntaxPlugin
{
    public function getType() { return 'substition'; }
    public function getPType() { return 'block'; }
    public function getSort() { return 160; }

    public function connectTo($mode)
    {
        $this->Lexer->addSpecialPattern('\{\{pedigree>[^}\n]*\}\}', $mode, 'plugin_awgenealogy_pedigree');
    }

    public function handle($match, $state, $pos, $handler)
    {
        $arg = substr($match, 11, -2);
        $opts = '';
        if (($q = strpos($arg, '?')) !== false) {
            $opts = substr($arg, $q + 1);
            $arg = substr($arg, 0, $q);
        }
        $data = ['root' => trim($arg), 'gens' => 5, 'redact' => false, 'blank' => false, 'tree' => false];
        foreach (explode('&', $opts) as $o) {
            $o = trim($o);
            if ($o === 'redact') $data['redact'] = true;
            elseif ($o === 'blank') $data['blank'] = true;
            elseif ($o === 'tree') $data['tree'] = true;
            elseif (ctype_digit($o) && (int)$o >= 2 && (int)$o <= 6) $data['gens'] = (int)$o;
        }
        return $data;
    }

    public function render($format, $renderer, $data)
    {
        if ($format !== 'xhtml') return true;
        global $ID, $INPUT;
        $renderer->info['cache'] = false; // the chart depends on other pages
        /** @var helper_plugin_awgenealogy $h */
        $h = plugin_load('helper', 'awgenealogy');

        if ($data['blank']) {
            $renderer->doc .= $h->blankHtml($data['gens']);
            return true;
        }
        $root = $data['root'];
        if ($root === '') $root = cleanID($INPUT->str('root'));
        if ($root === '') {
            $renderer->doc .= $h->pickerHtml($ID);
            return true;
        }
        // The form is the default view; tree and descendants are reading and print-friendly views.
        $view = $INPUT->str('awgview');
        if ($view === 'descendants') {
            if (!$data['redact']) {
                $renderer->doc .= $h->viewSwitch($ID, 'descendants', $INPUT->has('root') ? ['root' => $INPUT->str('root')] : []);
            }
            $renderer->doc .= $h->descendantsHtml($root, $data['gens'], $data['redact']);
            return true;
        }
        $tree = $data['redact'] || $data['tree'] ? true : ($view === 'tree');
        if ($view === 'form' && !$data['redact']) $tree = false;
        if ($tree) {
            if (!$data['redact']) $renderer->doc .= $h->viewSwitch($ID, 'tree', $INPUT->has('root') ? ['root' => $INPUT->str('root')] : []);
            $renderer->doc .= $h->chartHtml($root, $data['gens'], $data['redact']);
        } else {
            $renderer->doc .= $h->formHtml($root, $data['gens'], $ID, $INPUT->has('root') ? ['root' => $INPUT->str('root')] : []);
        }
        return true;
    }
}
