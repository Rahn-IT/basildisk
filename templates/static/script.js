document.addEventListener("DOMContentLoaded", () => {
    document.querySelectorAll('.erase-btn').forEach(btn => {
        btn.addEventListener('click', () => unfreeze_and_delete(btn.dataset.device));
    });
});


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
