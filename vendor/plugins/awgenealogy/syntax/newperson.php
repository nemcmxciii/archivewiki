<?php

use dokuwiki\Extension\SyntaxPlugin;

/** Small guided form for creating a person's page from the Genealogy start page. */
class syntax_plugin_awgenealogy_newperson extends SyntaxPlugin
{
    public function getType() { return 'substition'; }
    public function getPType() { return 'block'; }
    public function getSort() { return 160; }

    public function connectTo($mode)
    {
        $this->Lexer->addSpecialPattern('\{\{newperson>\}\}', $mode, 'plugin_awgenealogy_newperson');
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
        $renderer->doc .= $h->genealogyNavHtml('people') . '<form class="awg-newperson" method="post" action="#">' .
            '<input type="hidden" name="call" value="awgenealogy" />' .
            '<input type="hidden" name="mode" value="newperson" />' .
            '<input type="hidden" name="id" value="genealogy:start" />' .
            '<input type="hidden" name="sectok" value="' . hsc(getSecurityToken()) . '" />' .
            '<label class="awg-newperson-name"><span>Person’s name</span><input type="text" name="name" maxlength="120" required autocomplete="off" placeholder="For example, Jane Doe" /></label>' .
            '<details class="awg-newperson-details"><summary>Add dates and places (optional)</summary>' .
            '<div class="awg-pair"><label><span>Born</span><input type="text" name="born" placeholder="Year or date" /></label>' .
            '<label><span>Birthplace</span><input type="text" name="birthplace" /></label></div>' .
            '<div class="awg-pair"><label><span>Died</span><input type="text" name="died" placeholder="Year or date" /></label>' .
            '<label><span>Place of death</span><input type="text" name="deathplace" /></label></div>' .
            '<details class="awg-fact-sources"><summary>Add sources for dates and places</summary>' .
            '<div class="awg-pair"><label><span>Birth date source</span><input type="text" name="born_source" maxlength="500" placeholder="Record, archive, page number, or URL" /></label>' .
            '<label><span>Birthplace source</span><input type="text" name="birthplace_source" maxlength="500" placeholder="Record, archive, page number, or URL" /></label></div>' .
            '<div class="awg-pair"><label><span>Death date source</span><input type="text" name="died_source" maxlength="500" placeholder="Record, archive, page number, or URL" /></label>' .
            '<label><span>Place of death source</span><input type="text" name="deathplace_source" maxlength="500" placeholder="Record, archive, page number, or URL" /></label></div></details></details>' .
            '<div class="awg-newperson-result" role="status" aria-live="polite"></div>' .
            '<button type="submit" class="button">Create person page</button></form>';
        return true;
    }
}
