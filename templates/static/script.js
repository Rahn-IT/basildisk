document.addEventListener("DOMContentLoaded", () => {
    document.querySelectorAll('.unfreeze-btn').forEach(btn => {
        btn.addEventListener('click', () => unfreeze());
    });
    document.querySelectorAll('.erase-btn').forEach(btn => {
        btn.addEventListener('click', () => erase(btn.dataset));
    });
});

async function erase(button_data) {
    const modal = document.createElement('div');
    Object.assign(modal.style, {
        position: 'fixed',
        top: '0',
        left: '0',
        width: '100%',
        height: '100%',
        background: 'rgba(0, 0, 0, 0.5)',
        display: 'flex',
        justifyContent: 'center',
        alignItems: 'center'
    });

    const container = document.createElement('div');
    container.className = 'card';

    const message = document.createElement('p');
    message.textContent = "This will delete all data on this disk. YOU CANNOT RECOVER DELETED DATA AFTERWARDS";
    container.appendChild(message);
    const message2 = document.createElement('p');
    message2.textContent = "Are you sure?";
    container.appendChild(message2);

    const buttons = document.createElement('div')
    buttons.className = 'flex';

    const abort_btn = document.createElement('button');
    abort_btn.className = 'btn';
    abort_btn.textContent = "Abort";
    abort_btn.addEventListener('click', () => {
        document.body.removeChild(modal);
    });
    buttons.appendChild(abort_btn);

    const form = createForm("", button_data);
    const confirm_btn = document.createElement('input');
    confirm_btn.type = "submit";
    confirm_btn.className = 'btn';
    confirm_btn.value = "Confirm Secure Erase";
    form.appendChild(confirm_btn);
    buttons.appendChild(form);

    container.appendChild(buttons);

    modal.appendChild(container);
    document.body.appendChild(modal);
}

async function unfreeze() {
    let selection = await promptOptions("This will put the Machine to sleep and wake it up again. Depending on how fast this computer is, this might take up to a minute.", {
        "false": "Abort",
        "true": "Confirm"
    });

    if (selection === "true") {
        const modal = document.createElement('div');
        Object.assign(modal.style, {
            position: 'fixed',
            top: '0',
            left: '0',
            width: '100%',
            height: '100%',
            background: 'rgba(0, 0, 0, 0.5)',
            display: 'flex',
            justifyContent: 'center',
            alignItems: 'center'
        });

        const container = document.createElement('div');
        container.className = 'card';

        const message = document.createElement('p');
        message.textContent = "Machine is Sleeping, Please wait...";
        container.appendChild(message);

        modal.appendChild(container);
        document.body.appendChild(modal);

        fetch("/sleep", {
            method: "POST"
        });

        setTimeout(() => {
            window.location.reload()
        }, 10000)

    }
}

function createForm(url, data) {
    const form = document.createElement('form');
    form.method = 'POST';
    form.action = url;

    Object.keys(data).forEach(key => {
        const input = document.createElement('input');
        input.type = 'hidden';
        input.name = key;
        input.value = data[key];
        form.appendChild(input);
    });

    return form
}

function promptOptions(text, options) {
    return new Promise(resolve => {
        const modal = document.createElement('div');
        Object.assign(modal.style, {
            position: 'fixed',
            top: '0',
            left: '0',
            width: '100%',
            height: '100%',
            background: 'rgba(0, 0, 0, 0.5)',
            display: 'flex',
            justifyContent: 'center',
            alignItems: 'center'
        });

        const container = document.createElement('div');
        container.className = 'card';

        const message = document.createElement('p');
        message.textContent = text;
        container.appendChild(message);

        Object.keys(options).forEach(key => {
            const btn = document.createElement('button');
            btn.className = 'btn';
            btn.textContent = options[key];
            btn.addEventListener('click', () => {
                resolve(key);
                document.body.removeChild(modal);
            });
            container.appendChild(btn);
        });

        modal.appendChild(container);
        document.body.appendChild(modal);
    });
}
