<?php

use dokuwiki\Extension\ActionPlugin;
use dokuwiki\Extension\EventHandler;
use dokuwiki\Extension\Event;

/**
 * AJAX endpoint for the pedigree chart: add a missing parent, or edit a person's details.
 * @license GPL 2 (http://www.gnu.org/licenses/gpl.html)
 */
class action_plugin_awgenealogy extends ActionPlugin
{
    public function register(EventHandler $controller)
    {
        $controller->register_hook('AJAX_CALL_UNKNOWN', 'BEFORE', $this, 'handleAjax');
    }

    private function reply($ok, $msg, array $extra = [])
    {
        header('Content-Type: application/json; charset=utf-8');
        echo json_encode(array_merge(['ok' => $ok, 'message' => $msg], $extra));
    }

    public function handleAjax(Event $event, $param)
    {
        if ($event->data !== 'awgenealogy') return;
        $event->stopPropagation();
        $event->preventDefault();

        global $INPUT;
        /** @var helper_plugin_awgenealogy $h */
        $h = plugin_load('helper', 'awgenealogy');

        if (!checkSecurityToken()) return $this->reply(false, 'Security token expired. Reload the page and try again.');

        $mode = $INPUT->post->str('mode');
        $id = cleanID($INPUT->post->str('id'));
        if ($id === '' || !page_exists($id)) return $this->reply(false, 'That person page no longer exists.');
        if (auth_quickaclcheck($id) < AUTH_EDIT) return $this->reply(false, 'You don\'t have permission to edit this page.');
        if (checklock($id) !== false) return $this->reply(false, 'The page is being edited by someone else.');

        $values = [];
        foreach (helper_plugin_awgenealogy::EDITABLE as $k) {
            if ($INPUT->post->has($k)) $values[$k] = $h->cleanValue($INPUT->post->str($k));
        }

        if ($mode === 'update') {
            if (array_key_exists('name', $values) && $values['name'] === '') return $this->reply(false, 'A name is required.');
            if (!empty($values['died'])) $values['living'] = 'no';
            $text = $h->setFields(rawWiki($id), $values);
            saveWikiText($id, $text, 'Details edited from the pedigree chart');
            return $this->reply(true, 'Saved.');
        }

        if ($mode === 'add') {
            $role = $INPUT->post->str('role');
            if (!in_array($role, ['father', 'mother'], true)) return $this->reply(false, 'Unknown role.');
            $name = $values['name'] ?? '';
            if ($name === '') return $this->reply(false, 'A name is required.');

            $ns = getNS($id);
            $base = cleanID(($ns !== '' ? $ns . ':' : '') . $name);
            if ($base === '') return $this->reply(false, 'That name can\'t be used for a page.');
            $new = $base;
            for ($i = 2; page_exists($new); $i++) $new = $base . '_' . $i;
            if (auth_quickaclcheck($new) < AUTH_CREATE) return $this->reply(false, 'You don\'t have permission to create that page.');

            $fields = $values;
            unset($fields['name']);
            $fields['living'] = empty($values['died']) ? 'yes' : 'no';
            $fields['sex'] = $role === 'father' ? 'M' : 'F';
            saveWikiText($new, $h->newPersonText($new, $name, $fields + ['name' => $name]), 'Added from the pedigree chart');

            $ref = (getNS($new) === $ns) ? noNS($new) : $new;
            saveWikiText($id, $h->setFields(rawWiki($id), [$role => $ref]), 'Linked ' . $role . ' from the pedigree chart');
            return $this->reply(true, 'Added.', ['id' => $new]);
        }

        return $this->reply(false, 'Unknown action.');
    }
}
