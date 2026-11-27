/* ArchiveWiki Genealogy: fill in or edit a person right on the chart (no pop-up dialogs). */
jQuery(function () {
    var $ = jQuery;
    var personLookupId = 0;

    $('.awg-newperson').on('submit', function (e) {
        e.preventDefault();
        var $form = $(this), $button = $form.find('button[type=submit]'), $result = $form.find('.awg-newperson-result');
        $result.removeClass('awg-bad awg-good').text('');
        if (!$.trim($form.find('[name=name]').val())) {
            $result.addClass('awg-bad').text('Enter a name to continue.');
            $form.find('[name=name]').trigger('focus');
            return;
        }
        $button.prop('disabled', true).text('Creating…');
        $.post(DOKU_BASE + 'lib/exe/ajax.php', $form.serialize(), null, 'json')
            .done(function (r) {
                if (r && r.ok && r.url) { window.location.href = r.url; return; }
                $result.addClass('awg-bad').text((r && r.message) || 'Could not create the person page.');
                $button.prop('disabled', false).text('Create person page');
            })
            .fail(function (xhr) {
                $result.addClass('awg-bad').text('Could not create the person page (' + xhr.status + ').');
                $button.prop('disabled', false).text('Create person page');
            });
    });

    $('.awg-people-search').on('input', function () {
        var query = $.trim($(this).val().toLocaleLowerCase()), shown = 0, $list = $(this).closest('.awg-directory');
        $list.find('.awg-person-row').each(function () {
            var match = $(this).attr('data-search').toLocaleLowerCase().indexOf(query) !== -1;
            $(this).toggle(match);
            if (match) shown++;
        });
        $list.find('.awg-directory-count').text(shown + (shown === 1 ? ' person' : ' people'));
        $list.find('.awg-directory-none').prop('hidden', shown !== 0);
    });

    $('.awg-family-search').on('input', function () {
        var query = $.trim($(this).val().toLocaleLowerCase()), shown = 0, $list = $(this).closest('.awg-family-directory');
        $list.find('li[data-search]').each(function () {
            var match = $(this).attr('data-search').toLocaleLowerCase().indexOf(query) !== -1;
            $(this).toggle(match);
            if (match) shown++;
        });
        $list.find('.awg-family-count').text(shown + (shown === 1 ? ' family group' : ' family groups'));
        $list.find('.awg-family-none').prop('hidden', shown !== 0);
    });

    $('.awg-family-editor, .awg-formwrap').each(function () {
        var $container = $(this), people = [];
        try {
            people = JSON.parse($container.attr('data-people') || '[]');
        } catch (error) {
            $container.find('.awg-person-message').text('Person search is unavailable. You can still enter a new name.');
        }

        $container.find('.awg-person-lookup').each(function (index) {
            var $lookup = $(this), $input = $lookup.find('.awg-person-input'),
                $id = $lookup.find('.awg-person-id'), $list = $lookup.find('.awg-person-suggestions'),
                $message = $lookup.find('.awg-person-message'), linkOnly = $lookup.attr('data-link-only') === 'true';
            $list.attr('id', 'awg-person-options-' + (++personLookupId));
            $input.attr('aria-controls', $list.attr('id'));
            if ($id.val()) {
                $lookup.find('.awg-person-clear').prop('hidden', false);
                $message.text(linkOnly
                    ? 'This existing person will be linked; their details will not be changed.'
                    : 'This existing person is linked. Changes to the details below update their person page.');
            }
        });

        function selectedElsewhere(id, $current) {
            var selected = false;
            $container.find('.awg-person-id').not($current.find('.awg-person-id')).each(function () {
                if ($(this).val() === id) selected = true;
            });
            $container.find('.awg-card[data-id]').not($current.closest('.awg-card')).each(function () {
                if ($(this).attr('data-id') === id) selected = true;
            });
            return selected;
        }

        function closeList($lookup) {
            $lookup.find('.awg-person-suggestions').prop('hidden', true).empty();
            $lookup.find('.awg-person-input').attr('aria-expanded', 'false');
        }

        function showMatches($lookup, query) {
            var $list = $lookup.find('.awg-person-suggestions'), $input = $lookup.find('.awg-person-input');
            var matches = people.filter(function (person) {
                return person.name.toLocaleLowerCase().indexOf(query) !== -1 && !selectedElsewhere(person.id, $lookup);
            }).slice(0, 8);
            $lookup.data('matches', matches);
            $list.empty();
            if (!matches.length) {
                $list.append($('<div class="awg-person-no-match" role="option" aria-disabled="true"/>')
                    .text('No available match. You can still add a new person.'));
                $lookup.find('.awg-person-message').text($lookup.find('.awg-person-id').val()
                    ? 'No other match. The selected person remains linked.'
                    : 'No available match. Saving will create a new person page.');
            } else {
                matches.forEach(function (person, index) {
                    var details = [];
                    if (person.born) details.push('born ' + person.born);
                    if (person.died) details.push('died ' + person.died);
                    var label = person.name + (details.length ? ' · ' + details.join(', ') : '');
                    $('<button type="button" class="awg-person-option" role="option"/>')
                        .attr('data-index', index).text(label).appendTo($list);
                });
                $lookup.find('.awg-person-message').text('Choose a person to link their existing page.');
            }
            $list.prop('hidden', false);
            $input.attr('aria-expanded', 'true');
        }

        $container.on('input', '.awg-person-input', function () {
            var $input = $(this), $lookup = $input.closest('.awg-person-lookup'), query = $.trim($input.val()).toLocaleLowerCase();
            if (query.length < 2) {
                closeList($lookup);
                $lookup.find('.awg-person-message').text('Type at least two letters to search, or enter a new name.');
                return;
            }
            showMatches($lookup, query);
        }).on('keydown', '.awg-person-input', function (event) {
            var $lookup = $(this).closest('.awg-person-lookup');
            if (event.key === 'ArrowDown' && !$lookup.find('.awg-person-suggestions').prop('hidden')) {
                event.preventDefault();
                $lookup.find('.awg-person-option').first().trigger('focus');
            } else if (event.key === 'Escape') {
                closeList($lookup);
            }
        }).on('keydown', '.awg-person-option', function (event) {
            var $options = $(this).closest('.awg-person-suggestions').find('.awg-person-option');
            var index = $options.index(this);
            if (event.key === 'ArrowDown' && index < $options.length - 1) {
                event.preventDefault();
                $options.eq(index + 1).trigger('focus');
            } else if (event.key === 'ArrowUp') {
                event.preventDefault();
                if (index > 0) $options.eq(index - 1).trigger('focus');
                else $(this).closest('.awg-person-lookup').find('.awg-person-input').trigger('focus');
            } else if (event.key === 'Escape') {
                var $lookup = $(this).closest('.awg-person-lookup');
                closeList($lookup);
                $lookup.find('.awg-person-input').trigger('focus');
            }
        }).on('mousedown', '.awg-person-option', function (event) {
            event.preventDefault();
        }).on('click', '.awg-person-option', function () {
            var $lookup = $(this).closest('.awg-person-lookup');
            var person = ($lookup.data('matches') || [])[parseInt($(this).attr('data-index'), 10)];
            if (!person) return;
            var $record = $lookup.closest('.awg-family-member, .awg-child-row, .awg-card');
            $lookup.find('.awg-person-id').val(person.id);
            $lookup.find('.awg-person-input').val(person.name);
            ['name', 'born', 'birthplace', 'died', 'deathplace', 'sex'].forEach(function (field) {
                var $field = $record.find('[name$="[' + field + ']"]');
                if ($field.length) $field.val(person[field] || '');
            });
            if ($lookup.attr('data-link-only') === 'true') {
                $record.find('input[type=text]').prop('readonly', true);
                $lookup.find('.awg-person-message').text('This person will be linked; their details will not be changed.');
            } else {
                $lookup.find('.awg-person-message').text('This existing person is linked. Changes to the details below update their person page.');
            }
            $lookup.find('.awg-person-clear').prop('hidden', false);
            closeList($lookup);
            $lookup.find('input').trigger('change');
        }).on('click', '.awg-person-clear', function () {
            var $lookup = $(this).closest('.awg-person-lookup'), $record = $lookup.closest('.awg-family-member, .awg-child-row, .awg-card');
            var linkOnly = $lookup.attr('data-link-only') === 'true';
            $lookup.find('.awg-person-id').val('');
            $lookup.find('.awg-person-input').val('');
            $record.find('input[type=text]').val('').prop('readonly', false);
            $lookup.find('.awg-person-clear').prop('hidden', true);
            $lookup.find('.awg-person-input').trigger('input');
            $lookup.find('.awg-person-message').text(linkOnly
                ? 'Choose a match to link that person. If there is no match, saving will create a new person page.'
                : 'Choose a match to link an existing person. Otherwise, a new person page will be created when you save.');
            $lookup.find('.awg-person-input').trigger('focus');
        }).on('focusout', '.awg-person-input', function () {
            var $lookup = $(this).closest('.awg-person-lookup');
            setTimeout(function () { if (!$lookup.find('.awg-person-suggestions :focus').length) closeList($lookup); }, 100);
        });
    });

    $('.awg-family-editor').on('click', '.awg-add-child', function () {
        var $rows = $(this).siblings('.awg-child-rows'), $last = $rows.children('.awg-child-row').last();
        if ($rows.children('.awg-child-row').length >= 20) {
            $(this).prop('disabled', true);
            return;
        }
        var index = 0;
        $rows.find('[name^="children["]').each(function () {
            var match = $(this).attr('name').match(/^children\[(\d+)\]/);
            if (match) index = Math.max(index, parseInt(match[1], 10) + 1);
        });
        var $row = $last.clone();
        $row.find('[name]').each(function () {
            var $input = $(this), name = $input.attr('name').replace(/children\[\d+\]/, 'children[' + index + ']');
            $input.attr('name', name).val('');
        });
        $row.find('input[type=hidden]').val('');
        $row.find('.awg-person-input').val('');
        $row.find('.awg-person-clear').prop('hidden', true);
        $row.find('.awg-person-message').text('Choose an existing person to link them, or enter a name to create a person page when you save.');
        $row.find('.awg-person-suggestions').prop('hidden', true).empty();
        $row.find('input[type=text]').prop('readonly', false);
        var $picker = $row.find('.awg-person-lookup'), $suggestions = $picker.find('.awg-person-suggestions');
        $suggestions.attr('id', 'awg-person-options-' + (++personLookupId));
        $picker.find('.awg-person-input').attr('aria-controls', $suggestions.attr('id')).attr('aria-expanded', 'false');
        $rows.append($row);
        if ($rows.children('.awg-child-row').length >= 20) $(this).prop('disabled', true);
        $row.find('input[type=text]').first().trigger('focus');
    });

    $('.awg-family-editor').on('click', '.awg-remove-child', function () {
        var $group = $(this).closest('.awg-family-children'),
            $rows = $(this).closest('.awg-child-rows'), $row = $(this).closest('.awg-child-row');
        if ($rows.children('.awg-child-row').length > 1) $row.remove();
        else $row.find('input').val('');
        $group.find('.awg-add-child').prop('disabled', false);
    });

    $('.awg-family-form').each(function () {
        try {
            var message = sessionStorage.getItem('awgFamilySaved');
            if (message) {
                $(this).find('.awg-family-result').addClass('awg-good').text(message);
                sessionStorage.removeItem('awgFamilySaved');
            }
        } catch (ignore) {}
    }).on('submit', function (e) {
        e.preventDefault();
        var $form = $(this), $button = $form.find('button[type=submit]'), $result = $form.find('.awg-family-result');
        $result.removeClass('awg-bad awg-good').text('');
        if (!$.trim($form.find('[name=title]').val())) {
            $result.addClass('awg-bad').text('Give this family group a name.');
            $form.find('[name=title]').trigger('focus');
            return;
        }
        $button.prop('disabled', true).text('Saving…');
        $.post(DOKU_BASE + 'lib/exe/ajax.php', $form.serialize(), null, 'json')
            .done(function (r) {
                if (r && r.ok && r.url) {
                    try { sessionStorage.setItem('awgFamilySaved', r.message || 'Family group saved.'); } catch (ignore) {}
                    window.location.href = r.url;
                    return;
                }
                $result.addClass('awg-bad').text((r && r.message) || 'Could not save this family group.');
                $button.prop('disabled', false).text($form.find('[name=mode]').val() === 'newfamily' ? 'Create family group' : 'Save family group');
            })
            .fail(function (xhr) {
                $result.addClass('awg-bad').text('Could not save this family group (' + xhr.status + ').');
                $button.prop('disabled', false).text($form.find('[name=mode]').val() === 'newfamily' ? 'Create family group' : 'Save family group');
            });
    });

    function field(name, label, value) {
        return $('<input type="text" class="edit"/>').attr({name: name, placeholder: label, title: label}).val(value || '');
    }

    function openForm($box, mode, vals) {
        if ($box.data('awg-open')) return;
        $box.data('awg-open', true);
        var $old = $box.children().not('.awg-num').detach();
        var $f = $('<div class="awg-form"/>');
        var $name = field('name', 'Full name', vals.name);
        var $born = field('born', 'Born (date)', vals.born);
        var $bp = field('birthplace', 'Birthplace', vals.birthplace);
        var $died = field('died', 'Died (date)', vals.died);
        var $dp = field('deathplace', 'Place of death', vals.deathplace);
        var $err = $('<div class="awg-err"/>');
        var $save = $('<button type="button" class="button"/>').text('Save');
        var $cancel = $('<button type="button" class="button"/>').text('Cancel');
        $f.append($name, $('<div class="awg-row"/>').append($born, $bp), $('<div class="awg-row"/>').append($died, $dp), $err, $save, $cancel);
        $box.append($f);
        $box.closest('.awg-chart').css('overflow', 'visible');
        $name.focus();

        function close() {
            $f.remove();
            $box.append($old);
            $box.data('awg-open', false);
        }
        $cancel.on('click', close);
        $f.on('keydown', function (e) { if (e.which === 27) close(); if (e.which === 13) $save.click(); });
        $save.on('click', function () {
            $err.text('');
            if ($.trim($name.val()) === '') { $err.text('Please enter a name.'); return; }
            var data = {
                call: 'awgenealogy',
                mode: mode,
                id: mode === 'add' ? vals.child : vals.id,
                sectok: $box.closest('.awg-chart').data('sectok'),
                name: $name.val(), born: $born.val(), birthplace: $bp.val(), died: $died.val(), deathplace: $dp.val()
            };
            if (mode === 'add') data.role = vals.role;
            $save.prop('disabled', true);
            $.post(DOKU_BASE + 'lib/exe/ajax.php', data, null, 'json')
                .done(function (r) {
                    if (r && r.ok) { window.location.reload(); }
                    else { $err.text((r && r.message) || 'Could not save.'); $save.prop('disabled', false); }
                })
                .fail(function (xhr) { $err.text('Could not save (' + xhr.status + ').'); $save.prop('disabled', false); });
        });
    }

    $(document).on('click', '.awg-chart .awg-add', function (e) {
        e.preventDefault();
        var $box = $(this).closest('.awg-box');
        openForm($box, 'add', {child: $box.data('child'), role: $box.data('role')});
    });
    $(document).on('click', '.awg-chart .awg-edit', function (e) {
        e.preventDefault();
        var $box = $(this).closest('.awg-box');
        openForm($box, 'update', {
            id: $box.data('id'), name: $box.attr('data-name'), born: $box.attr('data-born'),
            birthplace: $box.attr('data-birthplace'), died: $box.attr('data-died'), deathplace: $box.attr('data-deathplace')
        });
    });

    // ---------------------------------------------------------------- fillable form view
    $('.awg-formwrap').each(function () {
        var $w = $(this);
        var $form = $w.find('form.awg-form');
        var $save = $form.find('.awg-save');
        var $msg = $form.find('.awg-msg');
        var $dirty = $form.find('.awg-dirty');
        var $sections = $form.find('.awg-gensec');
        var step = 0;

        function showStep(next) {
            step = Math.max(0, Math.min($sections.length - 1, next));
            $sections.hide().eq(step).show();
            $stepLabel.text('Generation ' + (step + 1) + ' of ' + $sections.length + ' · ' +
                $.trim($sections.eq(step).find('h3').text()).replace(/^Generation \d+ · /, ''));
            $previous.prop('disabled', step === 0);
            $next.text(step === $sections.length - 1 ? 'Last generation' : 'Next generation →')
                .prop('disabled', step === $sections.length - 1);
            $choices.find('button').removeClass('awg-current').attr('aria-current', null).eq(step)
                .addClass('awg-current').attr('aria-current', 'step');
        }

        var $steps = $('<nav class="awg-stepper" aria-label="Ancestor chart generations"/>');
        var $previous = $('<button type="button" class="button awg-step-prev">← Previous</button>');
        var $stepLabel = $('<strong class="awg-step-label" aria-live="polite"/>');
        var $next = $('<button type="button" class="button awg-step-next"/>');
        var $choices = $('<div class="awg-step-choices"/>');
        $sections.each(function (i) {
            var label = $.trim($(this).find('h3').text()).replace(/^Generation \d+ · /, '');
            var $choice = $('<button type="button"/>').text(i + 1).attr('aria-label', 'Show ' + label);
            $choice.on('click', function () { showStep(i); });
            $choices.append($choice);
        });
        $steps.append($previous, $choices, $stepLabel, $next);
        $sections.first().before($steps);
        $previous.on('click', function () { showStep(step - 1); });
        $next.on('click', function () { showStep(step + 1); });
        showStep(0);

        try {
            var savedMessage = sessionStorage.getItem('awgGenealogySaved');
            if (savedMessage) {
                $msg.addClass('awg-good').text(savedMessage);
                sessionStorage.removeItem('awgGenealogySaved');
            }
        } catch (ignore) {}

        function card(n) { return $form.find('.awg-card[data-n="' + n + '"]'); }
        function nameOf($c) { return $.trim($c.find('input[name$="[name]"]').val() || ''); }
        function hasPerson($c) { return !!$c.attr('data-id') || nameOf($c) !== ''; }

        // A card unlocks once the person below it (its child on the chart) has a name.
        function refresh() {
            $form.find('.awg-card').each(function () {
                var $c = $(this), n = parseInt($c.attr('data-n'), 10);
                if (n === 1 || $c.attr('data-id')) { return; }
                var $p = card(Math.floor(n / 2));
                var open = hasPerson($p) && !$p.hasClass('awg-blocked');
                $c.toggleClass('awg-blocked', !open);
                $c.find('input').prop('disabled', !open);
                $c.find('.awg-hint').text(open ? 'Not created yet' : 'Fill in the previous generation first.');
            });
        }
        refresh();

        $form.on('input change', 'input', function () {
            $save.prop('disabled', false);
            $dirty.text('Unsaved changes');
            $msg.removeClass('awg-bad awg-good').text('');
            refresh();
        });

        $form.on('submit', function (e) {
            e.preventDefault();
            $form.find('.awg-err').remove();
            $form.find('.awg-card').removeClass('awg-has-error');
            $msg.removeClass('awg-bad awg-good').text('');
            var data = $form.serializeArray();
            data.push(
                {name: 'call', value: 'awgenealogy'}, {name: 'mode', value: 'saveform'},
                {name: 'id', value: $w.attr('data-root')}, {name: 'gens', value: $w.attr('data-gens')},
                {name: 'sectok', value: $w.attr('data-sectok')}
            );
            $save.prop('disabled', true);
            $dirty.text('Saving…');
            $.post(DOKU_BASE + 'lib/exe/ajax.php', data, null, 'json')
                .done(function (r) {
                    if (r && r.ok) {
                        var saved = r.message || 'Changes saved.';
                        if (r.created || r.updated || r.linked) saved += ' ' +
                            (r.created ? r.created + (r.created === 1 ? ' person added. ' : ' people added. ') : '') +
                            (r.updated ? r.updated + (r.updated === 1 ? ' person updated. ' : ' people updated. ') : '') +
                            (r.linked ? r.linked + (r.linked === 1 ? ' existing person linked.' : ' existing people linked.') : '');
                        try { sessionStorage.setItem('awgGenealogySaved', saved); } catch (ignore) {}
                        window.location.reload();
                        return;
                    }
                    $msg.addClass('awg-bad').text((r && r.message) || 'Could not save.');
                    var first = null;
                    $.each((r && r.errors) || {}, function (n, text) {
                        var $c = card(n);
                        $c.addClass('awg-has-error').append($('<div class="awg-err"/>').text(text));
                        if (!first) { first = $c; }
                    });
                    if (first && first[0] && first[0].scrollIntoView) { first[0].scrollIntoView({block: 'center'}); }
                    $save.prop('disabled', false);
                    $dirty.text('Unsaved changes');
                })
                .fail(function (xhr) {
                    $msg.addClass('awg-bad').text('Could not save (' + xhr.status + ').');
                    $save.prop('disabled', false);
                    $dirty.text('Unsaved changes');
                });
        });
    });
});
