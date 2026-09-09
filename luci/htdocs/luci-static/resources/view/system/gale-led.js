'use strict';
'require dom';
'require form';
'require rpc';
'require ui';
'require view';

var GaleColorValue = form.Value.extend({
	renderWidget: function(sectionId, optionIndex, cfgvalue) {
		var node = this.super('renderWidget', [ sectionId, optionIndex, cfgvalue ]);
		var textInput = node.querySelector('input');
		var initial = /^#[0-9A-Fa-f]{6}$/.test(cfgvalue || '') ? cfgvalue.toUpperCase() : '#A020F0';
		var picker = E('input', {
			'type': 'color',
			'value': initial,
			'aria-label': _('Graphical color picker'),
			'style': 'width:4.5rem;height:2.4rem;padding:0.15rem;vertical-align:middle'
		});
		var preview = E('span', {
			'class': 'gale-led-preview',
			'title': _('Selected color preview'),
			'style': 'display:inline-block;width:2.4rem;height:2.4rem;margin-left:0.5rem;' +
				'vertical-align:middle;border:1px solid var(--border-color-medium,#888);border-radius:50%;' +
				'background:' + initial
		});

		textInput.setAttribute('maxlength', '7');
		textInput.setAttribute('placeholder', '#A020F0');
		textInput.setAttribute('spellcheck', 'false');
		textInput.style.width = '8rem';

		picker.addEventListener('input', function() {
			textInput.value = picker.value.toUpperCase();
			preview.style.backgroundColor = picker.value;
			textInput.dispatchEvent(new Event('change', { bubbles: true }));
		});
		textInput.addEventListener('input', function() {
			if (/^#[0-9A-Fa-f]{6}$/.test(textInput.value)) {
				picker.value = textInput.value;
				preview.style.backgroundColor = textInput.value;
			}
		});

		dom.append(node, [
			E('span', { 'style': 'display:inline-block;margin-left:0.5rem' }, [ picker, preview ])
		]);
		return node;
	}
});

var GaleBrightnessValue = form.RangeSliderValue.extend({
	formvalue: function(sectionId) {
		var element = this.getUIElement(sectionId);
		return element ? element.getValue().toString() : null;
	}
});

var callHardware = rpc.declare({
	object: 'gale-led',
	method: 'hardware',
	expect: { '': {} }
});

var callApply = rpc.declare({
	object: 'gale-led',
	method: 'apply',
	expect: { '': {} }
});

var callInterfaces = rpc.declare({
	object: 'gale-led',
	method: 'interfaces',
	expect: { '': {} }
});

return view.extend({
	load: function() {
		return Promise.all([
			L.resolveDefault(callHardware(), {
				supported: false,
				error: _('Unable to query Gale LED hardware')
			}),
			L.resolveDefault(callInterfaces(), { interfaces: [] })
		]);
	},

	render: function(data) {
		var hardware = data[0];
		var interfaces = data[1].interfaces || [];
		var m = new form.Map('gale-led', _('Google Wifi LED Control'),
			_('Configure a static color or a locally generated LED pattern on this Google Wifi Gale router.'));
		var s = m.section(form.NamedSection, 'main', 'led', _('LED settings'));
		var o;

		s.addremove = false;

		o = s.option(form.Flag, 'enabled', _('Enabled'),
			_('Apply the selected color. When disabled, all three LED channels are turned off.'));
		o.default = '1';
		o.rmempty = false;

		o = s.option(form.ListValue, 'mode', _('Pattern'),
			_('Static applies once. Animated and network patterns run under procd only while selected.'));
		o.value('static', _('Static color'));
		o.value('rainbow', _('Rainbow'));
		o.value('breathing', _('Breathing'));
		o.value('network_activity', _('Network activity'));
		o.value('network_heartbeat', _('Network heartbeat'));
		o.default = 'static';
		o.rmempty = false;

		o = s.option(GaleColorValue, 'color', _('Color'),
			_('Choose the static or pattern color graphically, or enter an exact #RRGGBB value.'));
		o.default = '#A020F0';
		o.validate = function(sectionId, value) {
			return /^#[0-9A-Fa-f]{6}$/.test(value)
				? true
				: _('Enter a color in the exact #RRGGBB form.');
		};
		o.rmempty = false;
		o.retain = true;
		o.depends('mode', 'static');
		o.depends('mode', 'breathing');
		o.depends('mode', 'network_activity');
		o.depends('mode', 'network_heartbeat');

		o = s.option(GaleBrightnessValue, 'brightness', _('Brightness'),
			_('Overall intensity applied proportionally to the red, green, and blue channels.'));
		o.default = '100';
		o.min = 0;
		o.max = 100;
		o.step = 1;
		o.datatype = 'range(0,100)';
		o.calcunits = '%';
		o.rmempty = false;

		o = s.option(form.ListValue, 'interface', _('Network interface'),
			_('Activity watches RX/TX byte counters. Heartbeat pulses while this interface is operational.'));
		interfaces.forEach(function(networkInterface) {
			o.value(networkInterface.name,
				'%s (%s)'.format(networkInterface.name, networkInterface.state || _('unknown')));
		});
		o.default = 'br-lan';
		o.rmempty = false;
		o.retain = true;
		o.depends('mode', 'network_activity');
		o.depends('mode', 'network_heartbeat');

		return m.render().then(function(formNode) {
			var notice;
			if (hardware.supported) {
				notice = E('div', { 'class': 'alert-message success' },
					_('Compatible Gale RGB LED hardware detected.'));
			}
			else {
				notice = E('div', { 'class': 'alert-message warning' }, [
					E('strong', {}, _('Compatible LED hardware was not detected.')),
					' ', hardware.error || _('All three LED0_Red, LED0_Green, and LED0_Blue channels are required.')
				]);
			}
			return E([ notice, formNode ]);
		});
	},

	handleSaveApply: function(ev, mode) {
		return this.handleSave(ev)
			.then(function() {
				return ui.changes.apply(mode == '0');
			})
			.then(function() {
				return callApply();
			})
			.then(function(result) {
				if (!result.ok)
					throw new Error(result.error || _('The LED configuration could not be applied'));
				ui.addTimeLimitedNotification(null,
					E('p', {}, _('Gale LED configuration applied successfully.')), 5000, 'info');
			})
			.catch(function(error) {
				ui.addNotification(null,
					E('p', {}, _('Failed to apply Gale LED configuration: %s').format(error.message || error)),
					'error');
				throw error;
			});
	}
});
